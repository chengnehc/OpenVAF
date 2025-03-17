use bitset::BitSet;

use crate::{DataFlowGraph, Inst, InstUseIter, Use, Value};

pub type PostorderParts<'a> = (BitSet<Inst>, Vec<(Inst, InstUseIter<'a>)>);

/// Postorder traversal of instructions in a data flow graph
///
/// Postorder traversal is when each node is visited after all of its
/// successors, except when the successor is only reachable by a back-edge
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
pub struct Postorder<'a, F> {
    dfg: &'a DataFlowGraph,
    pub visited: BitSet<Inst>,
    pub visit_stack: Vec<(Inst, InstUseIter<'a>)>,
    descend: F,
}

impl<'a, F: FnMut(Inst) -> bool> Postorder<'a, F> {
    pub fn new(dfg: &'a DataFlowGraph, descend: F) -> Postorder<'a, F> {
        Postorder {
            dfg,
            visited: BitSet::new_empty(dfg.num_insts()),
            visit_stack: Vec::new(),
            descend,
        }
    }

    pub fn from_parts(dfg: &'a DataFlowGraph, mut parts: PostorderParts<'a>, descend: F) -> Self {
        parts.0.ensure(dfg.num_insts());

        Postorder { dfg, visited: parts.0, visit_stack: parts.1, descend }
    }

    pub fn into_parts(self) -> PostorderParts<'a> {
        (self.visited, self.visit_stack)
    }

    pub fn clear(&mut self) {
        self.visited.clear();
    }

    pub fn populate(&mut self, val: Value) {
        for use_ in self.dfg.uses(val) {
            self.traverse_use(use_)
        }
    }

    pub fn traverse_successor(&mut self) {
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

impl<F: FnMut(Inst) -> bool> Iterator for Postorder<'_, F> {
    type Item = Inst;

    fn next(&mut self) -> Option<Inst> {
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
