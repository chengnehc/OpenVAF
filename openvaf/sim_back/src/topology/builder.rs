use ahash::AHashMap;
use bitset::BitSet;
use hir::CompilationDB;
use mir::builder::InstBuilder;
use mir::cursor::{Cursor, FuncCursor};
use mir::{Function, Inst, InstructionData, Opcode, Value, F_ZERO};

use super::{ContributeKind, Contribution, ImplicitEquation, Topology};

pub(super) struct Builder<'a> {
    pub(super) topology: Topology,
    pub(super) func: &'a mut Function,
    // references
    pub(super) db: &'a CompilationDB,
    pub(super) output_values: &'a BitSet<Value>,
    pub(super) op_dependent_insts: &'a BitSet<Inst>,
    pub(super) op_dependent_vals: &'a [Value],
    // temporary state
    pub(super) visited_set: BitSet<Inst>, // the set of instructions visited by postorder
    pub(super) val_map: AHashMap<Value, Value>, // for operator linearization
    pub(super) contrib_map: AHashMap<Value, ContributeKind>, // for topo building
}

// impl<'a> Builder<'a> {
//     pub(super) fn new(ctxt: &'a mut Context, topology: Topology) -> Self {
//         ctxt.init_op_dependent_insts();
//         let num_insts = ctxt.func.dfg.num_insts();

//         Builder {
//             topology,
//             db: ctxt.db,
//             func: &mut ctxt.func,
//             cfg: &mut ctxt.cfg,
//             output_values: &ctxt.output_values,
//             op_dependent_insts: &ctxt.op_dependent_insts,
//             op_dependent_vals: &ctxt.op_dependent_vals,
//             scratch_buf: BitSet::new_empty(num_insts),
//             postorder: Vec::with_capacity(128),
//             val_map: AHashMap::with_capacity(128),
//         }
//     }

//     pub(super) fn finish(mut self, intern: &'a mut HirInterner) {
//         let mut postdom_frontiers = SparseBitMatrix::new_square(self.func.layout.num_blocks());
//         ctxt.dom_tree.compute_postdom_frontiers(&self.cfg, &mut postdom_frontiers);
//         let operators = self.analog_operator_evaluations(&postdom_frontiers, &mut intern);
//         drop(postdom_frontiers);
//         self.build_analog_operators(operators, &mut intern);

//         mir_opt::simplify_cfg_no_phi_merge(self.func, self.cfg);
//         self.prune_small_signal();
//     }
// }

impl Builder<'_> {
    /// Turn one (or multiple) linear contributions into a separate dimension.
    ///
    /// This means:
    /// (1) the call instruction's result gets replaced with zero
    /// (2) all instructions that depends on it will turn to use its argument
    pub(super) fn create_dimension(&mut self, orig: Value, mapped: Value, postorder: &[Inst]) {
        self.val_map.clear();
        self.val_map.insert(orig, mapped);

        // temporary work stacks to hold phis and phi edges
        let mut phis = Vec::with_capacity(128);
        let mut edges = Vec::with_capacity(128);

        // reverse post-order traversal is a serialization of the DFG starting from
        // the call instruction of the analog operator
        for &inst in postorder.iter().rev() {
            macro_rules! ins {
                () => {
                    FuncCursor::new(self.func).after_inst(inst).ins()
                };
            }

            use {InstructionData::*, Opcode::*};
            let val = match self.func.dfg.insts[inst] {
                Unary { opcode: OptBarrier, arg } => {
                    let Some(&val) = self.val_map.get(&arg) else { continue };
                    val
                }
                Unary { opcode: Fneg, arg } => {
                    let Some(&val) = self.val_map.get(&arg) else { continue };
                    ins!().fneg(val)
                }
                Binary { opcode: Fadd, args } => {
                    match (self.val_map.get(&args[0]), self.val_map.get(&args[1])) {
                        (None, None) => continue,
                        (None, Some(&val)) | (Some(&val), None) => val,
                        (Some(&lhs), Some(&rhs)) => ins!().fadd(lhs, rhs),
                    }
                }
                Binary { opcode: Fsub, args } => {
                    match (self.val_map.get(&args[0]), self.val_map.get(&args[1])) {
                        (None, None) => continue,
                        (None, Some(&rhs)) => ins!().fneg(rhs),
                        (Some(&lhs), None) => lhs,
                        (Some(&lhs), Some(&rhs)) => ins!().fsub(lhs, rhs),
                    }
                }
                Binary { opcode: Fmul, args: [lhs, rhs] } => {
                    match (self.val_map.get(&lhs), self.val_map.get(&rhs)) {
                        (None, None) | (Some(_), Some(_)) => continue,
                        (None, Some(&rhs)) => ins!().fmul(lhs, rhs),
                        (Some(&lhs), None) => ins!().fmul(lhs, rhs),
                    }
                }
                Binary { opcode: Fdiv, args: [num, denom] } => {
                    let Some(&num) = self.val_map.get(&num) else { continue };
                    ins!().fdiv(num, denom)
                }
                PhiNode(_) => {
                    phis.push(inst);
                    // delay phi construction as there could be loops in the DFG
                    self.func.dfg.make_invalid_value()
                }
                _ => continue,
            };
            // insert the value mapping
            self.val_map.insert(self.func.dfg.first_result(inst), val);
        }
        // now that all values have been built we can populate the phis
        for inst in phis {
            let res = self.val_map[&self.func.dfg.first_result(inst)];
            let phi = self.func.dfg.insts[inst].unwrap_phi();
            edges.clear();
            for (bb, mut val) in
                phi.edges(&self.func.dfg.insts.value_lists, &self.func.dfg.phi_forest)
            {
                val = self.val_map.get(&val).copied().unwrap_or(F_ZERO);
                edges.push((bb, val));
            }
            FuncCursor::new(self.func).after_inst(inst).ins().with_result(res).phi(&edges);
        }
        // replace the original value with zero
        self.func.dfg.replace_uses(orig, F_ZERO);
    }

    pub(super) fn new_implicit_equation(
        &mut self,
        equation: ImplicitEquation,
        contrib: Contribution,
    ) {
        if contrib.resist != F_ZERO {
            self.contrib_map
                .insert(contrib.resist, ContributeKind::Implicit { id: equation, reactive: false });
        }
        if contrib.react != F_ZERO {
            self.contrib_map
                .insert(contrib.react, ContributeKind::Implicit { id: equation, reactive: true });
        }
        let eq = self.topology.implicit_equations.push_and_get_key(contrib);
        debug_assert_eq!(eq, equation);
    }

    pub(super) fn as_contribute_kind(&self, val: Value) -> Option<ContributeKind> {
        self.contrib_map.get(&val).copied()
    }
}
