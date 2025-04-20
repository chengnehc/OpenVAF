//! # Dominators
//!
//! - A block `d` is said to dominate/post-dominate block `n` if every path from the
//!   entry/exit block to `n` must go through `d`. Trivially, each block has at least
//!   one dominator: itself.
//! - A block `d` is said to strictly (post-)dominates block `n` if `d` (post-)dominates
//!   `n` and `d` does not equal to `n`.
//!
//! Dominators and post-dominators tell us which block(s) must be executed prior to, or
//! after a basic block.
//!
//! # Immediate Dominator
//!
//! - The immediate dominator(idom) of a block `n` is the *unique* block that strictly
//!   dominates `n` but does not strictly dominate any other block that strictly dominates
//!   `n`.
//! - By definition, every block, except for the entry block and unreachable blocks,
//!   has a unique immediate dominator.
//!
//! # Dominator Tree
//!
//! A tree where each node's children are those nodes it immediately (post-)dominates.
//! Each node's parent is its unique immediate (post-)dominator. The root node is the
//! entry/exit block.
//!
//! # Dominance frontiers
//!
//! Think of (post-)dominance frontiers as blocks that are “just before” or “just after”
//! the blocks we’re dominated by, or blocks we dominate.
//!
//! The dominance frontier of a basic block N, DF(N), is the set of all blocks, Y such
//! that N dominates a predecessor of Y but does not strictly dominate Y.
//! DF(N) = { Y | N dom pred(Y) AND !(N sdom Y)}
//!
//! See Also:
//! - https://docs.rs/crate/cranelift-codegen/latest/source/src/dominator_tree.rs

use std::cmp::Ordering;
use stdx::packed_option::PackedOption;

use bitset::SparseBitMatrix;
use typed_index_collections::{TiSlice, TiVec};

use crate::cfg::Successors;
use crate::{Block, ControlFlowGraph, Function};

mod render;

/// Dominator tree node, one for each block.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DomTreeNode {
    /// Number of this node in a (reverse) post-order traversal of the CFG, starting from `1`.
    /// This number is monotonic in the reverse postorder but not contiguous.
    /// Unreachable nodes get number `0`, all others are positive.
    rpo_number: u32,

    /// The immediate dominator of this block. This is `None` for unreachable blocks and the
    /// entry block which doesn't have an immediate dominator.
    idom: PackedOption<Block>,
}

const UNDEF: u32 = 0;
const DONE: u32 = 1;
const SEEN: u32 = 2;

/// A Dominator Tree represented as mappings of `Block`s to their immediate dominator.
#[derive(Default)]
pub struct DominatorTree {
    /// Mapping from a block to its possible dominator.
    nodes: TiVec<Block, DomTreeNode>,
    /// Mapping from a block to its possible post-dominator.
    reverse_nodes: TiVec<Block, DomTreeNode>,
    /// Post-order traversal of all reachable blocks.
    postorder: Vec<Block>,
    // Scratch memory used by `compute_postorder()`.
    stack: Vec<(Block, Successors)>,
}

/// Methods for querying the dominator tree.
impl DominatorTree {
    /// Get the CFG post-order traversal of blocks that was used to compute the dom_tree.
    ///
    /// Note that this post-order is not updated automatically when the CFG is modified.
    /// It is computed from scratch and cached by `compute()`.
    pub fn cfg_postorder(&self) -> &[Block] {
        &self.postorder
    }

    /// Returns the immediate dominator of `block`.
    pub fn idom(&self, block: Block) -> Option<Block> {
        self.nodes[block].idom.into()
    }

    /// Returns the immediate post-dominator of `block`.
    pub fn ipdom(&self, block: Block) -> Option<Block> {
        self.reverse_nodes[block].idom.into()
    }

    /// Does `dom` dominate `block`?
    pub fn dominates(&self, block: Block, dom: Block) -> bool {
        Self::dominates_(&self.nodes, block, dom)
    }

    /// Does `pdom` post-dominate `block`?
    pub fn post_dominates(&self, block: Block, pdom: Block) -> bool {
        Self::dominates_(&self.reverse_nodes, block, pdom)
    }

    fn dominates_(nodes: &TiSlice<Block, DomTreeNode>, mut block: Block, dominator: Block) -> bool {
        while nodes[block].rpo_number > nodes[dominator].rpo_number {
            let Some(parent) = nodes[block].idom.expand() else { return false };
            block = parent;
        }
        block == dominator
    }

    /// Compute the common dominator of two basic blocks.
    ///
    /// Both basic blocks are assumed to be reachable.
    fn common_dominator(
        nodes: &TiSlice<Block, DomTreeNode>,
        mut bb1: Block,
        mut bb2: Block,
    ) -> Block {
        loop {
            let rpo1 = nodes[bb1].rpo_number;
            let rpo2 = nodes[bb2].rpo_number;
            match rpo1.cmp(&rpo2) {
                Ordering::Less => bb2 = nodes[bb2].idom.expect("Unreachable basic block?"),
                Ordering::Greater => bb1 = nodes[bb1].idom.expect("Unreachable basic block?"),
                Ordering::Equal => return bb1,
            }
        }
    }
}

impl DominatorTree {
    /// Allocate a new blank dominator tree.
    pub fn new() -> Self {
        Default::default()
    }

    /// Allocate and compute a (post-)dominator tree using given `func` and `cfg`.
    pub fn with_func_and_cfg<const DOM: bool, const PDOM: bool>(
        func: &Function,
        cfg: &ControlFlowGraph,
    ) -> Self {
        let mut dom_tree = Self::new();
        dom_tree.compute::<DOM, PDOM>(func, cfg);
        dom_tree
    }

    /// Clear the data structures used to represent the dominator tree.
    pub fn clear(&mut self) {
        self.nodes.clear();
        self.reverse_nodes.clear();
        self.postorder.clear();
        debug_assert!(self.stack.is_empty());
    }

    /// Compute a CFG post-order traversal and dominator tree, or reverse post-order
    /// traversal and post-dominator tree.
    ///
    /// This operation will clear and reset the internal data structures.
    pub fn compute<const DOM: bool, const PDOM: bool>(
        &mut self,
        func: &Function,
        cfg: &ControlFlowGraph,
    ) {
        debug_assert!(cfg.is_valid());
        self.clear();
        if PDOM {
            self.compute_reverse_postorder(func, cfg);
            self.compute_domtree::<true>(cfg);
            self.postorder.clear(); // leave space for dominator computation
        }
        if DOM {
            self.compute_postorder(func, cfg);
            self.compute_domtree::<false>(cfg);
        }
    }

    /// Reset all internal data structures and compute a reverse post-order of the control flow graph.
    ///
    /// During this algorithm only, use `rpo_number` to hold the following state:
    ///
    ///   UNDEF: block has not yet been reached in the pre-order.
    ///   SEEN: block has been pushed on the stack but successors not yet pushed.
    ///   DONE: Successors pushed.
    ///
    /// # Note: the result is saved in `postorder`
    fn compute_reverse_postorder(&mut self, func: &Function, cfg: &ControlFlowGraph) {
        self.reverse_nodes
            .resize(func.layout.num_blocks(), DomTreeNode { rpo_number: UNDEF, idom: None.into() });

        let Some(block) = func.layout.last_block() else { return };
        self.stack.push((block, Successors::default()));
        self.reverse_nodes[block].rpo_number = SEEN;

        while let Some((block, _)) = self.stack.pop() {
            match self.reverse_nodes[block].rpo_number {
                SEEN => {
                    // This is the first time we pop the block, so we need to scan its successors and
                    // then revisit it.
                    self.reverse_nodes[block].rpo_number = DONE;
                    self.stack.push((block, Successors(None.into(), None.into())));

                    for block in cfg.pred_iter(block) {
                        if self.reverse_nodes[block].rpo_number == UNDEF {
                            self.reverse_nodes[block].rpo_number = SEEN;
                            self.stack.push((block, Successors(None.into(), None.into())));
                        }
                    }
                }
                DONE => {
                    // This is the second time we pop the block, so all successors have been
                    // processed.
                    self.postorder.push(block);
                }
                _ => unreachable!(),
            }
        }

        debug_assert_eq!(self.postorder.last().copied(), func.layout.last_block());
    }

    /// Reset all internal data structures and compute a post-order of the control flow graph.
    ///
    /// During this algorithm only, use `rpo_number` to hold the following state:
    ///
    ///   UNDEF:    block has not yet been reached in the pre-order.
    ///   SEEN: block has been pushed on the stack but successors not yet pushed.
    ///   DONE: Successors pushed.
    ///
    fn compute_postorder(&mut self, func: &Function, cfg: &ControlFlowGraph) {
        self.nodes
            .resize(func.layout.num_blocks(), DomTreeNode { rpo_number: UNDEF, idom: None.into() });

        let Some(block) = func.layout.entry_block() else { return };
        self.stack.push((block, cfg.successors_of(block)));
        self.nodes[block].rpo_number = SEEN;

        loop {
            while let Some(block) = self.stack.last_mut().and_then(|(_, succ)| succ.pop()) {
                if self.nodes[block].rpo_number == UNDEF {
                    self.nodes[block].rpo_number = SEEN;
                    self.stack.push((block, cfg.successors_of(block)))
                }
            }
            let Some((block, _)) = self.stack.pop() else { break };
            self.nodes[block].rpo_number = DONE;
            self.postorder.push(block)
        }
        debug_assert_eq!(self.postorder.last().copied(), func.layout.entry_block());
    }

    /// Build a dominator tree from a control flow graph using Keith D. Cooper's
    /// "Simple, Fast Dominator Algorithm."
    fn compute_domtree<const REVERSE: bool>(&mut self, cfg: &ControlFlowGraph) {
        // During this algorithm, `rpo_number` has the following values:
        //
        // 0: block is not reachable.
        // 1: block is reachable, but has not yet been visited during the first pass. This is set by
        // `compute_postorder`.
        // 2+: block is reachable and has an assigned RPO number.

        // We'll be iterating over a reverse post-order of the CFG, skipping the entry block.
        let Some((entry_block, postorder)) = self.postorder.as_slice().split_last() else { return };

        // Do a first pass where we assign RPO numbers to all reachable nodes.
        let nodes = if REVERSE { &mut self.reverse_nodes } else { &mut self.nodes };
        nodes[*entry_block].rpo_number = 2;
        for (rpo_idx, &block) in postorder.iter().rev().enumerate() {
            // Update the current node and give it an RPO number.
            // The entry block got 2, the rest start at 3
            //
            // Since `compute_idom` will only look at nodes with an assigned RPO number, the
            // function will never see an uninitialized predecessor.
            //
            // Due to the nature of the post-order traversal, every node we visit will have at
            // least one predecessor that has previously been visited during this RPO.
            let node = DomTreeNode {
                rpo_number: rpo_idx as u32 + 3,
                idom: self.compute_idom::<REVERSE>(block, cfg).into(),
            };

            let nodes = if REVERSE { &mut self.reverse_nodes } else { &mut self.nodes };
            nodes[block] = node;
        }

        // Iterate until convergence.
        //
        // If the function is free of irreducible control flow, this will exit after one iteration.
        let mut changed = true;
        while changed {
            changed = false;
            for &block in postorder.iter().rev() {
                let idom = self.compute_idom::<REVERSE>(block, cfg).into();
                let nodes = if REVERSE { &mut self.reverse_nodes } else { &mut self.nodes };
                if nodes[block].idom != idom {
                    nodes[block].idom = idom;
                    changed = true;
                }
            }
        }
    }

    fn compute_idom<const REVERSE: bool>(&self, block: Block, cfg: &ControlFlowGraph) -> Block {
        if REVERSE {
            Self::compute_idom_(&self.reverse_nodes, cfg.succ_iter(block))
        } else {
            Self::compute_idom_(&self.nodes, cfg.pred_iter(block))
        }
    }

    // Compute the immediate dominator for `block` using the current `idom` states for the reachable
    // nodes.
    fn compute_idom_(
        nodes: &TiSlice<Block, DomTreeNode>,
        preds: impl Iterator<Item = Block>,
    ) -> Block {
        // Get an iterator with just the reachable, already visited predecessors to `block`.
        // Note that during the first pass, `rpo_number` is 1 for reachable blocks that haven't
        // been visited yet, 0 for unreachable blocks.
        let mut reachable_preds = preds.filter(|bb| nodes[*bb].rpo_number > 1);

        // The RPO must visit at least one predecessor before this node.
        let mut idom =
            reachable_preds.next().expect("block node must have one reachable predecessor");

        for pred in reachable_preds {
            idom = Self::common_dominator(nodes, idom, pred);
        }

        idom
    }

    /// Compute the dominance frontiers of each block in the CFG, using Keith D. Cooper's
    /// "Simple, Fast Dominator Algorithm."
    ///
    /// The result is represented as a sparse square bitset, in which each row contains
    /// the dominance frontiers of a block.
    pub fn compute_dom_frontiers(
        &self,
        cfg: &ControlFlowGraph,
        dst: &mut SparseBitMatrix<Block, Block>,
    ) {
        dst.clear(self.nodes.len(), self.nodes.len());
        for bb in self.nodes.keys() {
            let mut predecessors = cfg.pred_iter(bb);
            // make sure the number of predecessors is at least 2
            let Some(first) = predecessors.next() else { continue };
            let Some(second) = predecessors.next() else { continue };
            Self::propagate_dom_frontiers(&self.nodes, first, bb, dst);
            Self::propagate_dom_frontiers(&self.nodes, second, bb, dst);
            for pred in predecessors {
                Self::propagate_dom_frontiers(&self.nodes, pred, bb, dst);
            }
        }
    }

    /// Compute the post dominance frontiers of each block in the CFG.
    pub fn compute_postdom_frontiers(
        &self,
        cfg: &ControlFlowGraph,
        dst: &mut SparseBitMatrix<Block, Block>,
    ) {
        dst.clear(self.reverse_nodes.len(), self.reverse_nodes.len());
        for bb in self.reverse_nodes.keys() {
            if let Some((bb1, bb2)) = cfg.successors_of(bb).as_pair() {
                Self::propagate_dom_frontiers(&self.reverse_nodes, bb1, bb, dst);
                Self::propagate_dom_frontiers(&self.reverse_nodes, bb2, bb, dst);
            }
        }
    }

    /// walk up the dominator tree until we reach the dominator of to
    fn propagate_dom_frontiers(
        nodes: &TiSlice<Block, DomTreeNode>,
        mut pos: Block,
        bb: Block,
        dst: &mut SparseBitMatrix<Block, Block>,
    ) {
        let end = nodes[bb].idom;
        while PackedOption::from(pos) != end {
            dst.insert(pos, bb);
            let Some(idom) = nodes[pos].idom.expand() else { break };
            pos = idom
        }
    }
}
