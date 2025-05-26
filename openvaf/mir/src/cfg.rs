//! A control flow graph represented as mappings of basic blocks to their predecessors
//! and successors.
//!
//! See Also:
//! - [cranelift_codegen::flowgraph](https://docs.rs/cranelift-codegen/latest/cranelift_codegen/flowgraph/index.html)

use std::iter::FilterMap;
use std::ops::Index;
use stdx::packed_option::PackedOption;

use typed_index_collections::TiVec;

use crate::{Block, Function, InstructionData};

//mod tests;
mod render;
mod traversal;

use traversal::{Postorder, ReversePostorder};

/// The Control Flow Graph maintains a mapping of blocks to their predecessors
/// and successors.
///
/// For each basic block, at least 1 predecessors and at most 2 successors are allowed.
/// This means that each node in the CFG has >=1 in-degree and 0, 1 or 2 out-degree.
#[derive(Clone, Default)]
pub struct ControlFlowGraph {
    /// Mapping of blocks to their predecessors and successors.
    data: TiVec<Block, CFGNode>,
    /// The memory pool to store predecessor sets.
    pred_forest: bforest::SetForest<Block>,
    /// Is this CFG valid?
    valid: bool,
}

/// A container for the successors and predecessors of some block.
#[derive(Clone, Default)]
pub struct CFGNode {
    pub predecessors: bforest::Set<Block>,
    pub successors: Successors,
}

/// Each `CFGNode` has at most two possible successor blocks.
#[derive(Clone, Default, Copy, PartialEq, Eq, Debug)]
pub struct Successors(PackedOption<Block>, PackedOption<Block>);

/// Iterators over block predecessors/successors.
pub type PredIter<'a> = bforest::SetIter<'a, Block>;
pub type PredRevIter<'a> = bforest::RevSetIter<'a, Block>;
pub type SuccIter = FilterMap<
    std::array::IntoIter<PackedOption<Block>, 2>,
    fn(PackedOption<Block>) -> Option<Block>,
>;

impl Successors {
    #[inline]
    pub fn is_empty(self) -> bool {
        self.0.is_none()
    }
    #[inline]
    pub fn contains(self, block: Block) -> bool {
        let expected = PackedOption::from(block);
        (self.0 == expected) | (self.1 == expected)
    }
    #[inline]
    pub fn iter(self) -> SuccIter {
        [self.0, self.1].into_iter().filter_map(|it| it.expand())
    }
    #[inline]
    pub fn as_array(self) -> [PackedOption<Block>; 2] {
        [self.0, self.1]
    }
    #[inline]
    pub fn as_pair(self) -> Option<(Block, Block)> {
        Some((self.0.expand()?, self.1.expand()?))
    }

    #[inline]
    pub fn clear(&mut self) {
        self.0 = None.into();
        self.1 = None.into();
    }
    #[inline]
    pub fn pop(&mut self) -> Option<Block> {
        self.1.take().or_else(|| self.0.take())
    }
    /// Insert `block` as a successor, return `true` when the successor
    /// has changed.
    #[inline]
    pub fn insert(&mut self, block: Block) -> bool {
        let res = PackedOption::from(block);
        if self.0.is_none() {
            self.0 = res;
        } else if self.0 == res {
            return false;
        } else if self.1.is_none() {
            self.1 = res;
        } else if self.1 == res {
            return false;
        } else if cfg!(debug_assertions) {
            unreachable!("no space to insert {block} into [{:?}, {:?}]", self.0, self.1);
        }

        true
    }
}

impl Index<Block> for ControlFlowGraph {
    type Output = CFGNode;

    fn index(&self, block: Block) -> &Self::Output {
        &self.data[block]
    }
}

/// Main APIs for operations on CFG.
impl ControlFlowGraph {
    /// Allocate a new blank control flow graph.
    pub fn new() -> Self {
        Default::default()
    }

    /// Allocate and compute the control flow graph for `func`.
    pub fn with_function(func: &Function) -> Self {
        let mut cfg = Self::new();
        cfg.compute(func);
        cfg
    }

    /// Clear all data structures in this control flow graph.
    pub fn clear(&mut self) {
        self.data.clear();
        self.pred_forest.clear();
        self.data.fill(CFGNode::default());
        self.valid = false;
    }

    /// Compute the control flow graph of `func` from scratch.
    ///
    /// # Note
    /// * This will clear and overwrite any information already stored in CFG.
    /// * This operation is expensive and `recompute_block` should be used when possible.
    pub fn compute(&mut self, func: &Function) {
        self.clear();
        self.data.resize(func.layout.num_blocks(), CFGNode::default());
        for block in &func.layout {
            self.compute_block(func, block);
        }
        self.valid = true;
    }

    fn compute_block(&mut self, func: &Function, block: Block) {
        if let Some(inst) = func.layout.last_inst(block) {
            match func.dfg.insts[inst] {
                InstructionData::Jump { destination } => self.add_edge(block, destination),
                InstructionData::Branch { then_dst, else_dst, .. } => {
                    // CAREFUL: Do not change the order of edges here.
                    // We want postorder traversal to always take the loop-free path.
                    self.add_edge(block, else_dst);
                    self.add_edge(block, then_dst); // `then_dst` might be a loop body
                }
                _ => (),
            }
        }
    }

    pub fn add_edge(&mut self, from: Block, to: Block) {
        self.data[from].successors.insert(to);
        self.data[to].predecessors.insert(from, &mut self.pred_forest, &());
    }

    /// Recompute the control flow graph of `block`.
    ///
    /// This is for use after modifying instructions within a specific block. It recomputes all edges
    /// from `block` while leaving edges to `block` intact. It functions as a subset of that of the
    /// more expensive `compute`, and should be used when we know we don't need to recompute the CFG
    /// from scratch, but rather that our changes have been restricted to specific blocks.
    pub fn recompute_block(&mut self, func: &Function, block: Block) {
        debug_assert!(self.is_valid());
        self.invalidate_block_successors(block);
        self.compute_block(func, block);
    }

    /// Replace block `old` with `new`.
    pub fn replace(&mut self, old: Block, new: Block) {
        debug_assert_ne!(old, new);
        debug_assert!(self.is_valid());

        let mut pos = self.data[old].predecessors.read_cursor();
        let old_ = old.into();
        let new_ = new.into();

        // update predecessors of `old` block
        while let Some(pred) = pos.next(&self.pred_forest) {
            let Successors(first, second) = &mut self.data[pred].successors;
            if *first == old_ {
                *first = new_;
                if *second == new_ {
                    second.take();
                }
            } else {
                debug_assert_eq!(*second, old_);
                if *first == new_ {
                    second.take();
                } else {
                    *second = new_;
                }
            }
            self.data[new].predecessors.insert(pred, &mut self.pred_forest, &());
        }
        self.data[old].predecessors.clear(&mut self.pred_forest);
        self.invalidate_block_successors(old);
    }

    fn invalidate_block_successors(&mut self, block: Block) {
        // Temporarily take ownership because we need mutable access to self.data inside the loop.
        // Unfortunately borrow checker cannot see that our mut accesses to predecessors don't alias
        // our iteration over successors.
        let successors = std::mem::take(&mut self.data[block].successors);
        for succ in successors.iter() {
            self.data[succ].predecessors.retain(&mut self.pred_forest, |pred| pred != block);
        }
    }

    pub fn ensure_block(&mut self, block: Block) {
        self.data.resize(usize::from(block) + 1, CFGNode::default())
    }

    /// Get an iterator over the predecessors to `block`.
    pub fn pred_iter(&self, block: Block) -> PredIter<'_> {
        self.data[block].predecessors.iter(&self.pred_forest)
    }

    /// Get an reverse iterator over the predecessors to `block`.
    pub fn pred_rev_iter(&self, block: Block) -> PredRevIter<'_> {
        self.data[block].predecessors.iter_rev(&self.pred_forest)
    }

    /// Get an iterator over the successors to `block`.
    pub fn succ_iter(&self, block: Block) -> SuccIter {
        debug_assert!(self.is_valid());
        self.data[block].successors.iter()
    }

    pub fn is_predecessor(&self, test_block: Block, this: Block) -> bool {
        self.data[this].predecessors.contains(test_block, &self.pred_forest, &())
    }

    #[inline]
    pub fn successors_of(&self, block: Block) -> Successors {
        self.data[block].successors
    }

    /// Returns the single, distinct successor of `block`, if any.
    #[inline]
    pub fn single_successor_of(&self, block: Block) -> Option<Block> {
        let mut iter = self.succ_iter(block);
        let res = iter.next()?;
        iter.next().is_none().then_some(res)
    }

    /// Returns the single, distinct predecessor of `block`, if any.
    #[inline]
    pub fn single_predecessor_of(&self, block: Block) -> Option<Block> {
        let mut iter = self.pred_iter(block);
        let res = iter.next()?;
        iter.next().is_none().then_some(res)
    }

    /// Test if there is a self loop, i.e. `block` only has itself as predecessor.
    #[inline]
    pub fn self_loop(&self, block: Block) -> bool {
        let mut iter = self.pred_iter(block);
        iter.all(|pred| pred == block)
    }

    /// Check if the CFG is in a valid state.
    ///
    /// Note that this doesn't perform any kind of validity checks. It simply checks if the
    /// `compute()` method has been called since the last `clear()`. It does not check that the
    /// CFG is consistent with the function.
    #[inline]
    pub fn is_valid(&self) -> bool {
        self.valid
    }

    #[inline]
    pub fn postorder_from(&self, start: Block) -> Postorder {
        Postorder::new(self, start)
    }

    #[inline]
    pub fn postorder(&self, func: &Function) -> Postorder {
        Postorder::new(self, func.layout.entry_block().unwrap())
    }

    #[inline]
    pub fn reverse_postorder_from(&self, start: Block) -> ReversePostorder {
        ReversePostorder::new(self, start)
    }

    #[inline]
    pub fn reverse_postorder(&self, func: &Function) -> ReversePostorder {
        ReversePostorder::new(self, func.layout.entry_block().unwrap())
    }
}
