//! Function layout.
//!
//! The order of basic blocks in a function and the order of instructions in a block is
//! determined by the `Layout` data structure defined in this module.

use std::{
    iter::{IntoIterator, Iterator},
    vec,
};
use stdx::packed_option::PackedOption;

use typed_index_collections::TiVec;

use crate::{Block, Inst};

#[cfg(test)]
mod tests;

/// The `Layout` struct determines the layout of blocks and instructions in a function. It does not
/// contain definitions of instructions or blocks, but depends on `Inst` and `Block` entity references
/// being defined elsewhere.
///
/// This data structure determines:
///
/// - The order of blocks in the function.
/// - Which block contains a given instruction.
/// - The order of instructions with a block.
///
/// While data dependencies are not recorded, instruction ordering does affect control
/// dependencies, so part of the semantics of the program are determined by the layout.
///
#[derive(Clone, Default)]
pub struct Layout {
    /// Linked list nodes for the layout order of blocks.
    /// Forms a doubly linked list, terminated in both ends by `None`.
    blocks: TiVec<Block, BlockNode>,

    /// Linked list nodes for the layout order of instructions.
    /// Forms a double linked list per block, terminated in both ends by `None`.
    insts: TiVec<Inst, InstNode>,

    /// First block in the layout order, or `None` when no blocks have been laid out.
    first_block: Option<Block>,

    /// Last block in the layout order, or `None` when no blocks have been laid out.
    last_block: Option<Block>,
}

impl Layout {
    pub fn new() -> Self {
        Default::default()
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
        self.insts.clear();
        self.first_block = None;
        self.last_block = None;
    }

    /// The capacity of basic blocks, not the actual number of blocks after MIR optimization
    pub fn num_blocks(&self) -> usize {
        self.blocks.len()
    }
}

/// Methods for laying out blocks.
///
/// An unknown block starts out as *not inserted* in the block layout. The layout is a linear order of
/// inserted blocks. Once a block has been inserted in the layout, instructions can be added. A block
/// can only be removed from the layout when it is empty.
///
/// Since every block must end with a terminator instruction which cannot fall through, the layout of
/// blocks do not affect the semantics of the program.
///
impl Layout {
    /// Create a new empty `Block`.
    #[inline]
    pub fn make_block(&mut self) -> Block {
        self.blocks.push_and_get_key(BlockNode::default())
    }

    #[inline]
    pub fn make_blocks(&mut self, num: usize) {
        self.blocks = TiVec::from(vec![BlockNode::default(); num]);
    }

    /// Is `block` currently part of the layout?
    #[inline]
    pub fn is_block_inserted(&self, block: Block) -> bool {
        Some(block) == self.first_block || self.blocks[block].prev.is_some()
    }

    /// Create and append a *new* empty `Block`.
    pub fn append_new_block(&mut self) -> Block {
        let block = self.make_block();
        self.append_block(block);

        block
    }

    /// Append `block` into function layout.
    pub fn append_block(&mut self, block: Block) {
        debug_assert!(
            !self.is_block_inserted(block),
            "Cannot append block that is already in the layout"
        );
        {
            let node = &mut self.blocks[block];
            debug_assert!(node.first_inst.is_none() && node.last_inst.is_none());
            node.prev = self.last_block.into();
            node.next = None.into();
        }
        if let Some(last) = self.last_block {
            self.blocks[last].next = block.into();
        } else {
            self.first_block = Some(block);
        }
        self.last_block = Some(block);
    }

    /// Insert `block` in the layout *before* the existing block `before`.
    pub fn insert_block(&mut self, block: Block, before: Block) {
        debug_assert!(
            !self.is_block_inserted(block),
            "Cannot insert block that is already in the layout"
        );
        debug_assert!(self.is_block_inserted(before), "block insertion point is not in the layout");
        let after = self.blocks[before].prev;
        {
            let node = &mut self.blocks[block];
            node.next = before.into();
            node.prev = after;
        }
        self.blocks[before].prev = block.into();
        match after.expand() {
            None => self.first_block = Some(block),
            Some(a) => self.blocks[a].next = block.into(),
        }
    }

    /// Insert `block` in the layout *after* the existing block `after`.
    pub fn insert_block_after(&mut self, block: Block, after: Block) {
        debug_assert!(
            !self.is_block_inserted(block),
            "Cannot insert block that is already in the layout"
        );
        debug_assert!(self.is_block_inserted(after), "block insertion point is not in the layout");
        let before = self.blocks[after].next;
        {
            let node = &mut self.blocks[block];
            node.next = before;
            node.prev = after.into();
        }
        self.blocks[after].next = block.into();
        match before.expand() {
            None => self.last_block = Some(block),
            Some(b) => self.blocks[b].prev = block.into(),
        }
    }

    pub fn clear_block(&mut self, block: Block) {
        let mut curr = self.first_inst(block);
        while let Some(inst) = curr {
            let n = &mut self.insts[inst];
            curr = n.next.expand();
            *n = InstNode::default();
        }
        self.blocks[block].first_inst = None.into();
        self.blocks[block].last_inst = None.into();
    }

    pub fn remove_empty_block(&mut self, block: Block) {
        debug_assert!(self.is_block_inserted(block), "block not in the layout");
        debug_assert!(self.first_inst(block).is_none(), "block must be empty.");

        // Extract links and clear the `block` node.
        let n = &mut self.blocks[block];
        let prev = n.prev;
        let next = n.next;
        n.prev = None.into();
        n.next = None.into();

        // Fix up links to `block`.
        match prev.expand() {
            None => self.first_block = next.expand(),
            Some(p) => self.blocks[p].next = next,
        }
        match next.expand() {
            None => self.last_block = prev.expand(),
            Some(n) => self.blocks[n].prev = prev,
        }

        if self.last_block.unwrap() == block {
            self.last_block = prev.expand();
        }
    }

    pub fn clear_and_remove_block(&mut self, block: Block) {
        self.clear_block(block);
        self.remove_empty_block(block);
    }

    /// Get the function's entry block, which is the first block in the layout order.
    pub fn entry_block(&self) -> Option<Block> {
        self.first_block
    }

    /// Get the function's last block in the layout order.
    pub fn last_block(&self) -> Option<Block> {
        self.last_block
    }

    /// Get the block preceding `block` in the layout order.
    pub fn prev_block(&self, block: Block) -> Option<Block> {
        self.blocks[block].prev.expand()
    }

    /// Get the block following `block` in the layout order.
    pub fn next_block(&self, block: Block) -> Option<Block> {
        self.blocks[block].next.expand()
    }

    /// Return an iterator over all blocks in layout order.
    pub fn blocks(&self) -> BlockIter<'_> {
        BlockIter { layout: self, cursor: self.block_cursor() }
    }

    pub fn block_cursor(&self) -> BlockCursor {
        BlockCursor { head: self.first_block.into(), tail: self.last_block.into() }
    }
}

#[derive(Clone, Debug, Default)]
struct BlockNode {
    prev: PackedOption<Block>,
    next: PackedOption<Block>,
    first_inst: PackedOption<Inst>,
    last_inst: PackedOption<Inst>,
}

pub struct BlockIter<'f> {
    layout: &'f Layout,
    cursor: BlockCursor,
}
impl Iterator for BlockIter<'_> {
    type Item = Block;

    fn next(&mut self) -> Option<Block> {
        self.cursor.next(self.layout)
    }
}

impl DoubleEndedIterator for BlockIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.cursor.next_back(self.layout)
    }
}

/// Use a layout reference in a for loop.
impl<'f> IntoIterator for &'f Layout {
    type Item = Block;
    type IntoIter = BlockIter<'f>;

    fn into_iter(self) -> BlockIter<'f> {
        self.blocks()
    }
}

pub struct BlockCursor {
    pub head: PackedOption<Block>,
    pub tail: PackedOption<Block>,
}

impl BlockCursor {
    pub fn next(&mut self, layout: &Layout) -> Option<Block> {
        let block = self.head.expand()?;
        if self.head == self.tail {
            self.head = None.into();
            self.tail = None.into();
        } else {
            self.head = layout.blocks[block].next
        }

        Some(block)
    }

    pub fn next_back(&mut self, layout: &Layout) -> Option<Block> {
        let block = self.tail.expand()?;
        if self.head == self.tail {
            self.head = None.into();
            self.tail = None.into();
        } else {
            self.tail = layout.blocks[block].prev
        }

        Some(block)
    }
}

/// Methods for arranging instructions.
///
/// An instruction starts out as *not inserted* in the layout. An instruction can be inserted into
/// a block at a given position.
impl Layout {
    /// Get the block containing `inst`, or `None` if `inst` is not inserted in the layout.
    pub fn inst_block(&self, inst: Inst) -> Option<Block> {
        self.insts.get(inst).and_then(|inst| inst.block.expand())
    }

    /// Fetch a block's first instruction.
    pub fn first_inst(&self, block: Block) -> Option<Inst> {
        self.blocks[block].first_inst.into()
    }

    /// Fetch a block's last instruction.
    pub fn last_inst(&self, block: Block) -> Option<Inst> {
        self.blocks[block].last_inst.into()
    }

    // TODO(JW): same as `last_inst()`
    /// Fetch the terminator of `block`, which is the last instruction within the block.
    pub fn block_terminator(&self, block: Block) -> Option<Inst> {
        self.blocks[block].last_inst.into()
    }

    /// Fetch the instruction following `inst`.
    pub fn next_inst(&self, inst: Inst) -> Option<Inst> {
        self.insts[inst].next.expand()
    }

    /// Fetch the instruction preceding `inst`.
    pub fn prev_inst(&self, inst: Inst) -> Option<Inst> {
        self.insts[inst].prev.expand()
    }

    /// Iterate over the instructions in `block` in layout order.
    pub fn block_insts(&self, block: Block) -> InstIter {
        let cursor = self.block_inst_cursor(block);
        InstIter { layout: self, cursor }
    }

    pub fn block_inst_cursor(&self, block: Block) -> InstCursor {
        InstCursor { head: self.first_inst(block).into(), tail: self.last_inst(block).into() }
    }

    /// Append `inst` to the end of `block`.
    pub fn append_inst_to_block(&mut self, inst: Inst, block: Block) {
        if self.insts.len() <= usize::from(inst) {
            self.insts.resize(usize::from(inst) + 1, InstNode::default())
        }

        debug_assert_eq!(self.inst_block(inst), None);
        debug_assert!(
            self.is_block_inserted(block),
            "Cannot append instructions to block not in layout"
        );

        let block_node = &mut self.blocks[block];
        {
            let inst_node = &mut self.insts[inst];
            inst_node.block = block.into();
            inst_node.prev = block_node.last_inst;
            debug_assert!(inst_node.next.is_none());
        }
        if block_node.first_inst.is_none() {
            block_node.first_inst = inst.into();
        } else {
            self.insts[block_node.last_inst.unwrap()].next = inst.into();
        }
        block_node.last_inst = inst.into();
    }

    /// Insert `inst` before the instruction `before` in the same block.
    pub fn prepend_inst(&mut self, inst: Inst, before: Inst) {
        if self.insts.len() <= usize::from(inst) {
            self.insts.resize(usize::from(inst) + 1, InstNode::default())
        }

        debug_assert_eq!(self.inst_block(inst), None);
        let block =
            self.inst_block(before).expect("Instruction before insertion point not in the layout");

        let after = self.insts[before].prev;
        let n = &mut self.insts[inst];
        n.block = block.into();
        n.next = before.into();
        n.prev = after;
        self.insts[before].prev = inst.into();
        match after.expand() {
            None => self.blocks[block].first_inst = inst.into(),
            Some(a) => self.insts[a].next = inst.into(),
        }
    }

    /// Insert `inst` after the instruction `after` in the same block.
    pub fn append_inst(&mut self, inst: Inst, after: Inst) {
        if self.insts.len() <= usize::from(inst) {
            self.insts.resize(usize::from(inst) + 1, InstNode::default())
        }

        debug_assert_eq!(self.inst_block(inst), None);
        let block =
            self.inst_block(after).expect("Instruction after insertion point not in the layout");

        let before = self.insts[after].next;
        let n = &mut self.insts[inst];
        n.block = block.into();
        n.next = before;
        n.prev = after.into();
        self.insts[after].next = inst.into();
        match before.expand() {
            None => self.blocks[block].last_inst = inst.into(),
            Some(prev) => self.insts[prev].prev = inst.into(),
        }
    }

    /// Remove `inst` from the layout.
    pub fn remove_inst(&mut self, inst: Inst) {
        let block = self.inst_block(inst).expect("Instruction already removed.");
        let n = &mut self.insts[inst];

        // Extract links and clear the `inst` node.
        let prev = n.prev;
        let next = n.next;
        *n = InstNode::default();

        // Fix up links to `inst`.
        match prev.expand() {
            None => self.blocks[block].first_inst = next,
            Some(p) => self.insts[p].next = next,
        }
        match next.expand() {
            None => self.blocks[block].last_inst = prev,
            Some(n) => self.insts[n].prev = prev,
        }
    }

    /// Merges `succ` ito `pred` by removing the terminator from `pred` and appending all
    /// instructions of `succ` to `pred`. Afterwards `succ` is removed from the layout.
    ///
    /// # Note
    /// It is up to the caller to ensure that this merge is valid:
    /// * No phis remain in `succ`
    /// * `pred` is terminated by a `jump` to `succ`
    /// * no other branches to `succ` remain
    pub fn merge_blocks(&mut self, pred: Block, succ: Block) {
        // remove branch instructions from `pred`
        if let Some(succ_start) = self.blocks[succ].first_inst.expand() {
            let term = self.block_terminator(pred).unwrap();
            let pred_end = self.prev_inst(term);

            self.insts[term] = InstNode::default();

            let mut cursor = self.block_inst_cursor(succ);
            while let Some(inst) = cursor.next(self) {
                self.insts[inst].block = pred.into();
            }

            if let Some(pred_end) = pred_end {
                // link up insts
                self.insts[pred_end].next = succ_start.into();
                self.insts[succ_start].prev = pred_end.into();
            } else {
                // predecessor only contained jmp
                // just update the block
                self.blocks[pred].first_inst = self.blocks[succ].first_inst;
            }
            self.blocks[pred].last_inst = self.blocks[succ].last_inst;
        } else {
            // successor is empty... Kind of odd but probably valid (collapse empty jump the
            // terminator). Just remove the branch
            self.remove_inst(self.last_inst(pred).unwrap())
        }

        self.blocks[succ].first_inst = None.into();
        self.blocks[succ].last_inst = None.into();

        // finally delete `succ`
        self.remove_empty_block(succ)
    }

    /// Split the block containing `before` in two.
    ///
    /// Insert `new_block` after the old block and move `before` and the following instructions to
    /// `new_block`:
    ///
    /// ```text
    /// old_block:
    ///     i1
    ///     i2
    ///     i3 << before
    ///     i4
    /// ```
    /// becomes:
    ///
    /// ```text
    /// old_block:
    ///     i1
    ///     i2
    /// new_block:
    ///     i3 << before
    ///     i4
    /// ```
    pub(crate) fn split_block(&mut self, new_block: Block, before: Inst) {
        let old_block =
            self.inst_block(before).expect("The `before` instruction must be in the layout");
        debug_assert!(!self.is_block_inserted(new_block));

        // Insert new_block after old_block.
        let next_block = self.blocks[old_block].next;
        let last_inst = self.blocks[old_block].last_inst;
        {
            let node = &mut self.blocks[new_block];
            node.prev = old_block.into();
            node.next = next_block;
            node.first_inst = before.into();
            node.last_inst = last_inst;
        }
        self.blocks[old_block].next = new_block.into();

        // Fix backwards link.
        if Some(old_block) == self.last_block {
            self.last_block = Some(new_block);
        } else {
            self.blocks[next_block.unwrap()].prev = new_block.into();
        }

        // Disconnect the instruction links.
        let prev_inst = self.insts[before].prev;
        self.insts[before].prev = None.into();
        self.blocks[old_block].last_inst = prev_inst;
        match prev_inst.expand() {
            None => self.blocks[old_block].first_inst = None.into(),
            Some(pi) => self.insts[pi].next = None.into(),
        }

        // Fix the instruction -> block pointers.
        let mut opt_i = Some(before);
        while let Some(i) = opt_i {
            debug_assert_eq!(self.insts[i].block.expand(), Some(old_block));
            self.insts[i].block = new_block.into();
            opt_i = self.insts[i].next.into();
        }
    }
}

#[derive(Clone, Debug, Default)]
struct InstNode {
    /// The Block containing this instruction, or `None` if the instruction is not yet inserted.
    block: PackedOption<Block>,
    prev: PackedOption<Inst>,
    next: PackedOption<Inst>,
}

/// Iterate over instructions in a block in layout order. See `Layout::block_insts()`.
#[derive(Clone)]
pub struct InstIter<'f> {
    layout: &'f Layout,
    cursor: InstCursor,
}

impl Iterator for InstIter<'_> {
    type Item = Inst;

    fn next(&mut self) -> Option<Inst> {
        self.cursor.next(self.layout)
    }
}

impl DoubleEndedIterator for InstIter<'_> {
    fn next_back(&mut self) -> Option<Inst> {
        self.cursor.next_back(self.layout)
    }
}

#[derive(Clone, Copy)]
pub struct InstCursor {
    pub head: PackedOption<Inst>,
    pub tail: PackedOption<Inst>,
}

impl InstCursor {
    pub fn next(&mut self, layout: &Layout) -> Option<Inst> {
        let inst = self.head.expand()?;
        if self.head == self.tail {
            self.head = None.into();
            self.tail = None.into();
        } else {
            self.head = layout.insts[inst].next;
        }
        Some(inst)
    }

    pub fn next_back(&mut self, layout: &Layout) -> Option<Inst> {
        let inst = self.tail.expand()?;
        if self.head == self.tail {
            self.head = None.into();
            self.tail = None.into();
        } else {
            self.tail = layout.insts[inst].prev;
        }
        Some(inst)
    }
}
