//! Sparse Conditional Constant Propagation:
//! - assumes blocks don’t execute until proven otherwise
//! - assumes values are constants until proven otherwise
//!
//! During propagation, we keep track of:
//! - Blocks: assume unexecuted until proven otherwise -- `executable_blocks`
//! - Variables: assume not executed (only with proof of assignments of a non-
//!   constant value do we assume not constant) -- `lattices`
//!
//! Lattice for representing variables (values):
//!
//! ``` text
//!         top ........... we've seen evidence that the variable can hold different values
//!       / /  \ \
//! all possible values ... we've seen evidence that the variable has been assigned a constant
//!       \ \  / /
//!        bottom ......... not executed
//! ```

use bitset::BitSet;
use mir::cfg::{ControlFlowGraph, Successors};
use mir::{Block, Function, Inst, InstructionData, Opcode, PhiNode, Value, ValueDef, FALSE, TRUE};
use typed_index_collections::TiVec;

use crate::simplify::SimplifyCtx;

const DEGREE_LIMIT: usize = 64;

#[cfg(test)]
mod tests;

pub fn sparse_conditional_constant_propagation(func: &mut Function, cfg: &ControlFlowGraph) {
    let num_blocks = func.layout.num_blocks();
    let lattices = (0..func.dfg.num_values())
        .map(|val| match func.dfg.value_def(val.into()) {
            ValueDef::Param(_) | ValueDef::Invalid => FlatSet::Top,
            ValueDef::Const(_) => FlatSet::Elem(val.into()),
            ValueDef::Result(_, _) => FlatSet::Bottom,
        })
        .collect();

    let solver = ConstSolver {
        func,
        cfg,
        lattices,
        executable_blocks: BitSet::new_empty(num_blocks),
        feasible_edges: vec![Successors::default(); num_blocks].into(),
        overdef_work_list: Vec::with_capacity(64),
        inst_work_list: Vec::with_capacity(64),
        block_work_list: Vec::with_capacity(64),
    };

    let (lattices, executable_blocks) = solver.solve().unwrap_or_default();

    // dbg!(&lattices, &executable_blocks);

    for (val, lattice) in lattices.iter_enumerated() {
        let ValueDef::Result(inst, _) = func.dfg.value_def(val) else { continue };
        match lattice {
            FlatSet::Elem(const_) => {
                // replace values with constants
                func.dfg.replace_uses(val, *const_);
                if func.dfg.inst_results(inst).len() == 1 {
                    func.dfg.zap_inst(inst);
                    func.layout.remove_inst(inst)
                }
            }
            FlatSet::Top | FlatSet::Bottom => {
                // remove zombie instructions of blocks that never executes
                if let Some(bb) = func.layout.inst_block(inst) {
                    if !executable_blocks.contains(bb) {
                        func.dfg.zap_inst(inst);
                        func.layout.remove_inst(inst)
                    }
                }
            }
        }
    }

    // break loops of dead blocks to make CFG simplification easier
    for bb in func.layout.blocks() {
        if !executable_blocks.contains(bb) {
            if let Some(last_inst) = func.layout.last_inst(bb) {
                if let InstructionData::Branch { cond, .. } = &mut func.dfg.insts[last_inst] {
                    *cond = FALSE;
                }
            }
        }
    }
}

/// Extends a type `T` with top and bottom elements to make it a partially ordered set
/// in which no value of `T` is comparable with any other.
///
/// ```text
///         top
///       / /  \ \
/// all possible values of `T`
///       \ \  / /
///        bottom
///```
/// Rules:
///
/// ``` text
/// any ∘ top = top
/// any ∘ tottom = any
/// T_i ∘ T_j = T_i,  where i == j
/// T_i ∘ T_j = top,  where i != j
/// ```
/// where `top` means overdefined value, `bottom` means undefined value,
/// `any` means any value in the lattice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlatSet {
    /// Value is marked 'overdefined', since it can have multiple possible versions
    /// and therefore cannot be evaluated at compile time.
    ///
    /// Function parameters are overdefined by nature.
    Top,

    /// Value is a compile-time known constant.
    Elem(Value),

    /// Value is 'undefined', since we have not yet seen a definition for it.
    Bottom,
}

pub struct ConstSolver<'a> {
    func: &'a mut Function,
    cfg: &'a ControlFlowGraph,
    // outputs
    lattices: TiVec<Value, FlatSet>,
    executable_blocks: BitSet<Block>,
    // states
    feasible_edges: TiVec<Block, Successors>,
    overdef_work_list: Vec<Inst>,
    inst_work_list: Vec<Inst>,
    block_work_list: Vec<Block>,
}

impl ConstSolver<'_> {
    pub fn solve(mut self) -> Option<(TiVec<Value, FlatSet>, BitSet<Block>)> {
        let entry = self.func.layout.entry_block()?;

        self.executable_blocks.insert(entry);
        self.block_work_list.push(entry);

        while !self.block_work_list.is_empty()
            || !self.inst_work_list.is_empty()
            || !self.overdef_work_list.is_empty()
        {
            // Super-extra-high-degree PHI nodes are unlikely to ever be marked constant,
            // and slow us down a lot. Just mark them overdefined.
            let should_eval_phis = |block| self.cfg.pred_iter(block).count() <= DEGREE_LIMIT;

            // separate overdef worklist to drive the solver to termination faster
            while let Some(inst) = self.overdef_work_list.pop() {
                if let Some(bb) = self.func.layout.inst_block(inst) {
                    if self.executable_blocks.contains(bb) {
                        self.eval(bb, inst, should_eval_phis);
                    }
                }
            }
            while let Some(inst) = self.inst_work_list.pop() {
                if let Some(bb) = self.func.layout.inst_block(inst) {
                    if self.executable_blocks.contains(bb) {
                        self.eval(bb, inst, should_eval_phis);
                    }
                }
            }
            while let Some(bb) = self.block_work_list.pop() {
                let eval_phis = should_eval_phis(bb);
                let mut cursor = self.func.layout.block_inst_cursor(bb);
                while let Some(inst) = cursor.next(&self.func.layout) {
                    self.eval(bb, inst, |_| eval_phis);
                }
            }
        }

        Some((self.lattices, self.executable_blocks))
    }

    fn eval(&mut self, bb: Block, inst: Inst, should_eval_phis: impl FnOnce(Block) -> bool) {
        match self.func.dfg.insts[inst].clone() {
            InstructionData::Unary { opcode, arg } => self.eval_unary(opcode, arg, inst),
            InstructionData::Binary { opcode, args } => {
                self.eval_binary(opcode, args[0], args[1], inst)
            }
            InstructionData::Branch { cond, then_dst, else_dst, .. } => match self.lattices[cond] {
                FlatSet::Elem(TRUE) => self.mark_edge_feasible(bb, then_dst),
                FlatSet::Elem(FALSE) => self.mark_edge_feasible(bb, else_dst),
                _ => {
                    self.mark_edge_feasible(bb, then_dst);
                    self.mark_edge_feasible(bb, else_dst);
                }
            },
            InstructionData::Jump { destination } => {
                self.mark_edge_feasible(bb, destination);
            }
            InstructionData::PhiNode(phi) => {
                let eval_phi = should_eval_phis(bb);
                self.eval_phi(phi, bb, inst, eval_phi)
            }
            InstructionData::Call { .. } => {
                // work around the borrow checker
                let res = self.func.dfg.inst_results(inst);
                for i in 0..res.len() {
                    let res = self.func.dfg.inst_results(inst);
                    self.mark_overdefined(res[i]);
                }
            }
        }
    }

    fn eval_unary(&mut self, op: Opcode, arg: Value, inst: Inst) {
        let lattice = self.lattices[arg];
        match lattice {
            FlatSet::Bottom => (),
            FlatSet::Elem(arg) => {
                // TODO(JW): will always fall into const_eval, so this mapping is useless
                //
                // let mut simplify_ctx = SimplifyCtx::<f64, _>::new(self.func, |val, _| {
                //     if let FlatSet::Elem(const_val) = self.lattices[val] {
                //         const_val
                //     } else {
                //         val
                //     }
                // });
                let mut simplify_ctx = SimplifyCtx::<f64, _>::new(self.func, |val, _| val);
                if let Some(val) = simplify_ctx.simplify_unary_op(op, arg) {
                    debug_assert!(self.func.dfg.value_def(val).as_const().is_some());
                    self.mark_inst_const(inst, val)
                } else {
                    // this only happens when the instruction is an optbarrier
                    self.mark_inst_overdefined(inst)
                }
            }
            FlatSet::Top => self.mark_inst_overdefined(inst),
        }
    }

    fn eval_binary(&mut self, op: Opcode, lhs: Value, rhs: Value, inst: Inst) {
        let (lhs, rhs, overdef) = match (self.lattices[lhs], self.lattices[rhs]) {
            (FlatSet::Top, FlatSet::Elem(rhs)) => (lhs, rhs, true),
            (FlatSet::Elem(lhs), FlatSet::Top) => (lhs, rhs, true),
            (FlatSet::Bottom, FlatSet::Top) | (FlatSet::Top, FlatSet::Bottom | FlatSet::Top) => {
                (lhs, rhs, true)
            }
            (FlatSet::Elem(lhs), FlatSet::Elem(rhs)) => (lhs, rhs, false),
            (FlatSet::Elem(lhs), FlatSet::Bottom) => (lhs, rhs, false),
            (FlatSet::Bottom, FlatSet::Elem(rhs)) => (lhs, rhs, false),
            (FlatSet::Bottom, FlatSet::Bottom) => (lhs, rhs, false),
        };

        let mut simplify_ctx = SimplifyCtx::<f64, _>::new(self.func, |val, _| {
            if let FlatSet::Elem(const_val) = self.lattices[val] {
                const_val
            } else {
                val
            }
        });

        if let Some(val) = simplify_ctx.simplify_binop(op, lhs, rhs) {
            if self.func.dfg.value_def(val).as_const().is_some() {
                self.mark_inst_const(inst, val);
                return;
            }
        }

        if overdef {
            self.mark_inst_overdefined(inst)
        }
    }

    fn eval_phi(&mut self, phi: PhiNode, bb: Block, inst: Inst, eval_phi: bool) {
        let res = self.func.dfg.first_result(inst);
        let mut lattice = self.lattices[res];

        if lattice == FlatSet::Top {
            return;
        } else if !eval_phi {
            self.mark_overdefined(res);
            return;
        }

        for (pred, val) in self.func.dfg.phi_edges(&phi) {
            if !self.feasible_edges[pred].contains(bb) {
                continue;
            }

            let incoming_lattice = self.lattices[val];
            match (incoming_lattice, lattice) {
                (FlatSet::Top, _) => {
                    self.mark_overdefined(res);
                    return;
                }
                (_, FlatSet::Top) => {
                    debug_assert!(false, "loop should have breaked earlier");
                    return;
                }
                (FlatSet::Elem(old), FlatSet::Elem(new)) if new != old => {
                    self.mark_overdefined(res);
                    return;
                }
                _ => {
                    lattice = incoming_lattice;
                }
            }
        }

        if let FlatSet::Elem(val) = lattice {
            self.mark_const(res, val)
        }
    }

    fn mark_inst_const(&mut self, inst: Inst, val: Value) {
        let res = self.func.dfg.first_result(inst);
        self.mark_const(res, val);
    }

    fn mark_const(&mut self, dst: Value, val: Value) {
        if !matches!(self.lattices[dst], FlatSet::Elem(_)) {
            // dbg!(&self.lattices[dst]);
            self.lattices[dst] = FlatSet::Elem(val);
            for use_ in self.func.dfg.uses(dst) {
                let inst = self.func.dfg.use_to_user(use_);
                self.inst_work_list.push(inst);
            }
        }
    }

    fn mark_inst_overdefined(&mut self, inst: Inst) {
        let res = self.func.dfg.first_result(inst);
        self.mark_overdefined(res);
    }

    fn mark_overdefined(&mut self, val: Value) {
        if self.lattices[val] != FlatSet::Top {
            // dbg!(&self.lattices[val]);
            self.lattices[val] = FlatSet::Top;
            for use_ in self.func.dfg.uses(val) {
                let inst = self.func.dfg.use_to_user(use_);
                self.overdef_work_list.push(inst);
            }
        }
    }

    fn mark_edge_feasible(&mut self, src: Block, dst: Block) {
        if !self.feasible_edges[src].insert(dst) {
            return; // return if edge already inserted
        }
        if self.executable_blocks.insert(dst) {
            self.block_work_list.push(dst)
        } else {
            // block is already executable but we added  a new predecessor, `src`,
            // for `dst`, so we need to revisit phis (and dependent values)
            let eval_phis = self.cfg.pred_iter(dst).count() <= DEGREE_LIMIT;
            let mut cursor = self.func.layout.block_inst_cursor(dst);
            while let Some(inst) = cursor.next(&self.func.layout) {
                if let InstructionData::PhiNode(phi) = self.func.dfg.insts[inst].clone() {
                    self.eval_phi(phi, dst, inst, eval_phis)
                } else {
                    break;
                }
            }
        }
    }
}
