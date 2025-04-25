//! Aggressive DCE that takes control flow into account. Basic idea:
//! assume a statement is dead until proven otherwise.
//!
//! See also: Todd C. Mowry, Lecture 14: SSA-Style Optimizations, CMU

use bitset::{BitSet, SparseBitMatrix};
use mir::{Block, ControlFlowGraph, Function, Inst, InstructionData, Value, ValueDef};

#[cfg(test)]
mod tests;

pub type PostDominanceFrontiers = SparseBitMatrix<Block, Block>;

pub fn aggressive_dead_code_elimination(
    func: &mut Function,
    cfg: &mut ControlFlowGraph,
    is_live: &dyn Fn(Value, &Function) -> bool,
    pdf: &PostDominanceFrontiers,
) {
    let num_blocks = func.layout.num_blocks();

    let mut adce = AggressiveDeadCode {
        live_insts: BitSet::new_empty(func.dfg.num_insts()),
        live_blocks: BitSet::new_empty(num_blocks),
        live_predecessors: BitSet::new_empty(num_blocks),
        live_control_flow: BitSet::new_empty(num_blocks),
        inst_work_list: Vec::new(),
        bb_work_list: Vec::with_capacity(64),
        func,
        cfg,
        pdf,
    };

    adce.live_blocks.insert(func.layout.entry_block().unwrap());

    for inst in func.dfg.insts.iter() {
        if func.layout.inst_block(inst).is_some()
            && (func.dfg.has_side_effects(inst, false)
                || func.dfg.inst_results(inst).iter().any(|val| is_live(*val, func)))
        {
            adce.mark_inst_live(inst);
        }
    }

    let (mut live_insts, mut live_blocks) = adce.solve();

    let dead_insts = {
        live_insts.inverse();
        live_insts
    };
    let dead_blocks = {
        live_blocks.inverse();
        live_blocks
    };

    for inst in dead_insts.iter() {
        func.dfg.zap_inst(inst);
        if func.layout.inst_block(inst).is_some() && !func.dfg.insts[inst].is_terminator() {
            func.layout.remove_inst(inst);
        }
    }

    // JW: instructions in `dead_blocks` should also be in `dead_insts`, so they should
    // have been removed above (except for terminators).
    // So I suppose this is meant to make the following CFG simplification easier.
    for bb in dead_blocks.iter() {
        // debug_assert!(func.layout.block_insts(bb).all(|inst| dead_insts.contains(inst)));
        if let Some(term) = func.layout.last_inst(bb) {
            if let InstructionData::Branch { else_dst, .. } = func.dfg.insts[term] {
                func.dfg.insts[term] = InstructionData::Jump { destination: else_dst };
                cfg.recompute_block(func, bb);
            }
        }
    }
}

struct AggressiveDeadCode<'a> {
    // outputs
    live_insts: BitSet<Inst>,
    live_blocks: BitSet<Block>,
    // states
    live_predecessors: BitSet<Block>,
    live_control_flow: BitSet<Block>,
    inst_work_list: Vec<Inst>,
    bb_work_list: Vec<Block>,
    // mutates
    func: &'a Function,
    cfg: &'a ControlFlowGraph,
    // reads
    pdf: &'a PostDominanceFrontiers,
}

impl AggressiveDeadCode<'_> {
    pub fn solve(mut self) -> (BitSet<Inst>, BitSet<Block>) {
        loop {
            // if S is live, then its operands should be live
            while let Some(inst) = self.inst_work_list.pop() {
                for arg in self.func.dfg.instr_args(inst) {
                    if let ValueDef::Result(def, _) = self.func.dfg.value_def(*arg) {
                        self.mark_inst_live(def);
                    }
                }
            }

            // if S is live, then if T determines whether S executes, T should be live
            while let Some(bb) = self.bb_work_list.pop() {
                if let Some(control_deps) = self.pdf.row(bb) {
                    for dep in control_deps.iter() {
                        self.mark_term_live(dep)
                    }
                }
            }

            if self.inst_work_list.is_empty() {
                break (self.live_insts, self.live_blocks);
            }
        }
    }

    fn mark_inst_live(&mut self, inst: Inst) -> bool {
        if self.live_insts.insert(inst) {
            self.inst_work_list.push(inst);

            // JW: is it always safe to unwrap here?
            let bb = self.func.layout.inst_block(inst).unwrap();
            self.mark_bb_live(bb);

            if matches!(self.func.dfg.insts[inst], InstructionData::PhiNode(_))
                && self.live_predecessors.insert(bb)
            {
                for pred in self.cfg.pred_iter(bb) {
                    if self.live_control_flow.insert(pred) {
                        self.bb_work_list.push(pred)
                    }
                }
            }

            true
        } else {
            false
        }
    }

    fn mark_bb_live(&mut self, bb: Block) {
        if self.live_blocks.insert(bb) {
            if self.live_control_flow.insert(bb) {
                self.bb_work_list.push(bb);
            }

            // only deal with unconditional jmp
            if let Some(term) = self.func.layout.last_inst(bb) {
                if matches!(self.func.dfg.insts[term], InstructionData::Jump { .. }) {
                    self.live_insts.insert(term);
                    // no need to insert jmp into the work list etc.,
                    // for jmp does not have any operand
                }
            }
        }
    }

    fn mark_term_live(&mut self, bb: Block) {
        if let Some(term) = self.func.layout.last_inst(bb) {
            if let InstructionData::Branch { then_dst, else_dst, .. } = self.func.dfg.insts[term] {
                if self.mark_inst_live(term) {
                    self.mark_bb_live(then_dst);
                    self.mark_bb_live(else_dst);
                }
            }
        }
    }
}
