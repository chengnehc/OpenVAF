use std::mem;

use crate::instructions::{PhiEdges, PhiNode};
use crate::{Block, DataFlowGraph, Inst, Value, GRAVESTONE};

impl DataFlowGraph {
    pub fn phi_edges(&self, phi: &PhiNode) -> PhiEdges {
        phi.edges(&self.insts.value_lists, &self.phi_forest)
    }

    pub fn phi_edge_val(&self, phi: &PhiNode, pred: Block) -> Option<Value> {
        phi.edge_val_of(pred, &self.insts.value_lists, &self.phi_forest)
    }

    pub fn phi_eq(&self, phi1: &PhiNode, phi2: &PhiNode) -> bool {
        phi1.eq(phi2, &self.insts.value_lists, &self.phi_forest)
    }

    /// Insert `(block, val)` as an edge of phi instruction `inst`.
    /// If the edge has been inserted before, update it with `val`, otherwise create a new one.
    ///
    /// # Panics
    /// Panics when `inst` is not a phi.
    #[inline]
    pub fn insert_phi_edge(&mut self, inst: Inst, block: Block, val: Value) {
        let PhiNode { mut blocks, mut args } = self.insts[inst].unwrap_phi().clone();
        blocks.update_or_insert_with(
            block,
            |pos| {
                if let Some(pos) = pos {
                    let use_ = self.insts.operands(inst)[*pos as usize];
                    self.values.detach_use(use_, &self.insts);
                    args.as_mut_slice(&mut self.insts.value_lists)[*pos as usize] = val;
                    self.values.attach_use(use_, val);
                    *pos
                } else {
                    let pos = args.push(val, &mut self.insts.value_lists) as u32;
                    let use_ = self.values.make_use(val, inst, pos as u16);
                    self.insts.uses[inst].push(use_, &mut self.insts.use_lists);
                    pos
                }
            },
            &mut self.phi_forest,
            &(),
        );

        self.insts[inst] = PhiNode { blocks, args }.into();
    }

    #[inline]
    pub fn try_remove_phi_edge_at(&mut self, inst: Inst, block: Block) -> Option<Value> {
        let PhiNode { blocks, .. } = self.insts[inst].as_phi_mut()?;
        let pos = blocks.remove(block, &mut self.phi_forest, &())?;
        self.detach_operand(inst, pos as u16);

        // this use might be reattached again so we replace the value with a constant where
        // uses currently don't matter that much
        // TODO introduce dedicated gravestone value
        let val = mem::replace(&mut self.instr_args_mut(inst)[pos as usize], GRAVESTONE);

        Some(val)
    }

    #[inline]
    pub fn try_remove_phi_edge(
        &mut self,
        PhiNode { blocks, args }: &mut PhiNode,
        // _inst: Inst,
        block: Block,
    ) -> Option<Value> {
        let pos = blocks.remove(block, &mut self.phi_forest, &())?;
        // self.detach_operand(inst, pos as u16);

        let val = mem::replace(
            &mut args.as_mut_slice(&mut self.insts.value_lists)[pos as usize],
            GRAVESTONE,
        );

        Some(val)
    }
}
