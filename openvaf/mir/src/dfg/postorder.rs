use bitset::BitSet;

use super::{DataFlowGraph, Inst, InstUseIter, Use, Value};

/// Postorder traversal of instructions in a data flow graph.
///
/// Each node is visited after all of its successors, except
/// when the successor is only reachable by a back-edge.
///
/// ```text
///
///         A
///        / \
///       /   \
///      B     C
///       \   /
///        \ /
///         D
/// ```
///
/// A Postorder traversal of this graph is `D B C A` or `D C B A`
///
pub struct Postorder<'a, P: FnMut(Inst) -> bool> {
    dfg: &'a DataFlowGraph,
    /// a predicate that indicates whether to descend at a specific instruction
    descend: P,
    /// a bitset registering all visited instructions
    visited: BitSet<Inst>,
    /// the output of the postorder traserval
    visit_stack: Vec<(Inst, InstUseIter<'a>)>,
}

impl<'a, P: FnMut(Inst) -> bool> Postorder<'a, P> {
    pub fn new(dfg: &'a DataFlowGraph, descend: P) -> Postorder<'a, P> {
        Postorder {
            dfg,
            descend,
            visited: BitSet::new_empty(dfg.num_insts()),
            visit_stack: Vec::new(),
        }
    }

    pub fn visited(self) -> BitSet<Inst> {
        self.visited
    }

    pub fn at_value(mut self, val: Value) -> Self {
        self.populate(val);
        self.traverse_successor();
        self
    }

    pub fn at_inst(mut self, inst: Inst) -> Self {
        for &res in self.dfg.inst_results(inst) {
            self.populate(res);
        }
        self.traverse_successor();
        self
    }

    pub fn clear(&mut self) {
        self.visited.clear();
    }

    fn populate(&mut self, val: Value) {
        for use_ in self.dfg.uses(val) {
            self.traverse_use(use_)
        }
    }

    fn traverse_successor(&mut self) {
        while let Some(use_) = self.visit_stack.last_mut().and_then(|(_, iter)| iter.next()) {
            self.traverse_use(use_);
        }
    }

    fn traverse_use(&mut self, use_: Use) {
        let inst = self.dfg.use_to_user(use_);
        self.traverse_inst(inst);
    }

    fn traverse_inst(&mut self, inst: Inst) {
        if (self.descend)(inst) && self.visited.insert(inst) {
            self.visit_stack.push((inst, self.dfg.inst_uses(inst)));
        }
    }
}

impl<P: FnMut(Inst) -> bool> Iterator for Postorder<'_, P> {
    type Item = Inst;

    fn next(&mut self) -> Option<Self::Item> {
        let (inst, _) = self.visit_stack.pop()?;
        self.traverse_successor();

        Some(inst)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let lower = self.visit_stack.len();
        // All the blocks, minus the number of blocks we've visited.
        let upper = self.dfg.num_insts() - self.visited.count();

        (lower, Some(upper))
    }
}
