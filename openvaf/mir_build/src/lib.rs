//! MIR function builder.
//!
//! Provides a straightforward way to create a MIR function and fill it with
//! instructions corresponding to Verilog-A.
//!
//! Here is how it should be used when translating to MIR:
//!
//! - for each basic block, create a corresponding data for SSA construction with `declare_block`;
//! - while traversing a basic block and translating instruction, use `def_var` and `use_var`
//!   to record definitions and uses of variables, these methods will give you the corresponding
//!   SSA values;
//! - when all the instructions in a basic block have been translated, the block is said _filled_
//!   and only then you can add it as a predecessor to other blocks with `declare_block_predecessor`;
//! - when you have constructed all the predecessor to a basic block, call `seal_block` on it
//!   with the `Function` that you are building.
//!
//! See Also:
//!
//! https://docs.rs/cranelift-frontend/latest/cranelift_frontend/

use stdx::{impl_debug_display, impl_idx_from};

use lasso::Rodeo;
use mir::builder::{InsertBuilder, InstBuilder, InstInserterBase};
use mir::cursor::{Cursor, FuncCursor};
use mir::{
    Block, ControlFlowGraph, DataFlowGraph, FuncRef, Function, FunctionSignature, Ieee64, Inst,
    InstructionData, Param, Value,
};
use typed_index_collections::TiVec;

mod ssa;
use ssa::SSABuilder;

/// An opaque reference to a place. It could be viewed as a Variable or
/// a (possibly mutable) memory location.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Place(u32);
impl_idx_from!(Place(u32));
impl_debug_display!(
    match Place { Place(i) => "place{i}";}
);

pub struct FunctionBuilder<'a> {
    /// The function currently being built.
    /// This field is public so the function can be re-borrowed.
    pub func: &'a mut Function,
    /// An interner for string literals
    pub strlit: &'a mut Rodeo,
    /// Source location to assign to all new instructions.
    srcloc: mir::SourceLoc,

    func_ctxt: &'a mut FunctionBuilderContext,
    /// Block that this builder is currently at.
    position: Block,
    /// Block that is the end of this function.
    end: Block,
    /// for simulator backend
    tag_writes: bool,
}

/// Structure used for translating a series of functions into MIR.
///
/// In order to reduce memory reallocations when compiling multiple functions,
/// `FunctionBuilderContext` holds various data structures which are cleared between
/// functions, rather than dropped, preserving the underlying allocations.
#[derive(Default)]
pub struct FunctionBuilderContext {
    ssa: SSABuilder<ssa::IncompleteCfg>,
    status: TiVec<Block, BlockStatus>,
}

impl FunctionBuilderContext {
    /// Creates a `FunctionBuilderContext` structure. The structure is automatically
    /// cleared after each `FunctionBuilder` completes translating a function.
    pub fn new() -> Self {
        Self::default()
    }

    fn clear(&mut self) {
        self.ssa.clear();
        self.status.clear();
    }

    fn is_empty(&self) -> bool {
        self.ssa.is_empty() && self.status.is_empty()
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
enum BlockStatus {
    /// No instructions have been added.
    #[default]
    Empty,
    /// Some instructions have been added, but no terminator.
    Partial,
    /// A terminator instruction has been added; no further instruction may be added.
    Filled,
}

/// Implementation of the [`InstInserterBase`] trait that has
/// one convenience method per MIR instruction.
pub struct FuncInstBuilder<'short, 'long: 'short> {
    builder: &'short mut FunctionBuilder<'long>,
    block: Block,
}

impl<'short, 'long> FuncInstBuilder<'short, 'long> {
    fn new(builder: &'short mut FunctionBuilder<'long>, block: Block) -> Self {
        Self { builder, block }
    }
}

impl<'short> InstInserterBase<'short> for FuncInstBuilder<'short, '_> {
    fn data_flow_graph(&self) -> &DataFlowGraph {
        &self.builder.func.dfg
    }

    fn data_flow_graph_mut(&mut self) -> &mut DataFlowGraph {
        &mut self.builder.func.dfg
    }

    fn insert_built_inst(self, inst: Inst) -> &'short mut DataFlowGraph {
        // We only insert the Block in the layout when an instruction is added to it
        self.builder.ensure_inserted_block();
        self.builder.func.layout.append_inst_to_block(inst, self.block);
        self.builder.func.srclocs.push(self.builder.srcloc);

        match self.builder.func.dfg.insts[inst] {
            InstructionData::Branch { then_dst, else_dst, .. } => {
                self.builder.declare_successor(then_dst);
                self.builder.declare_successor(else_dst);
                self.builder.fill_current_block()
            }
            InstructionData::Jump { destination } => {
                self.builder.declare_successor(destination);
                self.builder.fill_current_block()
            }
            _ => (),
        }

        &mut self.builder.func.dfg
    }
}

pub trait RetBuilder {
    fn ret(self) -> Inst;
}

impl<'short> RetBuilder for InsertBuilder<'short, FuncInstBuilder<'short, '_>> {
    fn ret(self) -> Inst {
        let exit = self.inserter.builder.func.layout.exit_block().unwrap();
        self.jump(exit)
    }
}

/// The module is parametrized by one type which is the representation of variables in your
/// origin language. It offers a way to conveniently append instruction to your program flow.
/// You are responsible to split your instruction flow into extended blocks (declared with
/// `create_block`) whose properties are:
///
/// - branch and jump instructions can only point at the top of extended blocks;
/// - the last instruction of each block is a terminator instruction which has no natural successor,
///   and those instructions can only appear at the end of extended blocks.
///
/// The parameters of Cranelift IR instructions are Cranelift IR values, which can only be created
/// as results of other Cranelift IR instructions. To be able to create variables redefined multiple
/// times in your program, use the `def_var` and `use_var` command, that will maintain the
/// correspondence between your variables and Cranelift IR SSA values.
///
/// The first block for which you call `switch_to_block` will be assumed to be the beginning of
/// the function.
///
/// # Errors
///
/// The functions below will panic in debug mode whenever you try to modify the MIR
/// function in a way that violate the coherence of the code. For instance: switching to a new
/// `Block` when you haven't filled the current one with a terminator instruction, inserting a
/// return instruction with arguments that don't match the function's signature.
impl<'a> FunctionBuilder<'a> {
    /// Creates a new FunctionBuilder structure that will operate on a `Function` using a
    /// `FunctionBuilderContext`.
    pub fn new(
        func: &'a mut Function,
        strlit: &'a mut Rodeo,
        func_ctxt: &'a mut FunctionBuilderContext,
        tag_writes: bool,
    ) -> Self {
        debug_assert!(func_ctxt.is_empty());

        // entry and exit are always empty to allow for easy prepending/appending
        // JW: this would cause the exit block to always be block1
        let entry = func.layout.append_new_block();
        func_ctxt.status.push(BlockStatus::Empty);
        func_ctxt.ssa.declare_block();

        let exit = func.layout.append_new_block();
        func_ctxt.status.push(BlockStatus::Empty);
        func_ctxt.ssa.declare_block();

        let mut builder = Self {
            func,
            srcloc: Default::default(),
            strlit,
            func_ctxt,
            position: entry,
            end: exit,
            tag_writes,
        };
        builder.seal_block(entry);

        builder
    }

    /// Edit an existing `Function`. Return the `FunctionBuilder` and the terminator instruction.
    pub fn edit(
        func: &'a mut Function,
        strlit: &'a mut Rodeo,
        func_ctxt: &'a mut FunctionBuilderContext,
        tag_writes: bool,
    ) -> (Self, Inst) {
        debug_assert!(func_ctxt.is_empty());
        let Some(mut entry) = func.layout.entry_block() else {
            // Editing with an empty function is the same as creating a new one
            let builder = Self::new(func, strlit, func_ctxt, tag_writes);
            let term =
                builder.func.dfg.make_inst(InstructionData::Jump { destination: builder.end });
            return (builder, term);
        };

        let mut exit = func.layout.exit_block().unwrap();
        if exit == entry {
            exit = func.layout.append_new_block();
            FuncCursor::new(func).at_bottom(entry).ins().jump(exit);
        }

        let term = match func.layout.first_inst(entry) {
            Some(_) => {
                let old_entry = entry;
                entry = func.layout.make_block();
                func.layout.insert_block(entry, old_entry);
                func.dfg.make_inst(InstructionData::Jump { destination: old_entry })
            }
            None => func.dfg.make_inst(InstructionData::Jump { destination: exit }),
        };

        for _bb in 0..func.layout.num_blocks() {
            func_ctxt.status.push(BlockStatus::Empty);
            func_ctxt.ssa.declare_block();
        }

        let mut builder = Self {
            func,
            srcloc: Default::default(),
            strlit,
            func_ctxt,
            position: entry,
            end: exit,
            tag_writes,
        };
        builder.seal_block(entry);

        (builder, term)
    }

    /// Get the source location that should be assigned to all new instructions.
    pub fn get_srcloc(&self) -> mir::SourceLoc {
        self.srcloc
    }

    /// Set the source location that should be assigned to all new instructions.
    pub fn set_srcloc(&mut self, srcloc: mir::SourceLoc) {
        self.srcloc = srcloc;
    }

    /// Get the block that this builder is currently at.
    pub fn current_block(&self) -> Block {
        self.position
    }

    /// Creates a new `Block` and returns its reference.
    pub fn create_block(&mut self) -> Block {
        let block = self.func.layout.make_block();
        self.func_ctxt.status.push(BlockStatus::Empty);
        self.func_ctxt.ssa.declare_block();
        block
    }

    /// After the call to this function, new instructions will be inserted into the designated
    /// block, in the order they are declared.
    ///
    /// When inserting the terminator instruction (which doesn't have a fallthrough to its immediate
    /// successor), the block will be declared filled and it will not be possible to append
    /// instructions to it.
    pub fn switch_to_block(&mut self, block: Block) {
        // First we check that the previous block has been filled.
        debug_assert!(
            self.is_unreachable()
                || self.is_pristine(self.position)
                || self.is_filled(self.position),
            "you have to fill your block before switching"
        );
        // We cannot switch to a filled block
        debug_assert!(
            !self.is_filled(block),
            "you cannot switch to a block which is already filled"
        );

        // Then we change the cursor position.
        self.position = block;
    }

    /// Declares that all the predecessors of this block are known.
    ///
    /// Function to call with `block` as soon as the last branch instruction to `block` has been
    /// created. Forgetting to call this method on every block will cause inconsistencies in the
    /// produced functions.
    pub fn seal_block(&mut self, block: Block) {
        self.func_ctxt.ssa.seal_block(block, self.func);
    }

    /// Effectively calls seal_block on all unsealed blocks in the function.
    ///
    /// It's more efficient to seal `Block`s as soon as possible, during
    /// translation, but for frontends where this is impractical to do, this
    /// function can be used at the end of translating all blocks to ensure
    /// that everything is sealed.
    pub fn seal_all_blocks(&mut self) {
        self.func_ctxt.ssa.seal_all_blocks(self.func);
    }

    /// Ensure that the block at current position is inserted into the layout, as well as sealed.
    pub fn ensured_sealed(&mut self) {
        self.ensure_inserted_block();
        if !self.func_ctxt.ssa.is_sealed(self.position) {
            self.seal_block(self.position)
        }
    }

    /// Ensure that the block at current position is inserted in the layout.
    pub fn ensure_inserted_block(&mut self) {
        let block = self.position;
        if self.is_pristine(block) {
            if !self.func.layout.is_block_inserted(block) {
                self.func.layout.insert_block(block, self.end)
            }
            self.func_ctxt.status[block] = BlockStatus::Partial;
        } else {
            debug_assert!(
                !self.is_filled(block),
                "you cannot add an instruction to block {}: already filled\n{}",
                self.position,
                self.func.to_debug_string()
            );
        }
    }

    /// Register a new definition of a user variable to the current block.
    ///
    /// The type of the value must be the same as the type registered for the variable.
    pub fn def_var(&mut self, var: Place, val: Value) {
        if self.tag_writes {
            self.func.dfg.set_tag(val, Some(u32::from(var).into()));
        }
        self.func_ctxt.ssa.def_var(var, val, self.position);
    }

    /// Register a new definition of a user variable to the specified `block`.
    ///
    /// The type of the value must be the same as the type registered for the variable.
    pub fn def_var_at(&mut self, var: Place, val: Value, block: Block) {
        if self.tag_writes {
            self.func.dfg.set_tag(val, Some(u32::from(var).into()));
        }
        self.func_ctxt.ssa.def_var(var, val, block);
    }

    /// Returns the MIR value corresponding to the utilization at the current program
    /// position of a previously defined user variable.
    pub fn use_var(&mut self, var: Place) -> Value {
        self.ensure_inserted_block();
        self.func_ctxt.ssa.use_var(self.func, var, self.position)
    }

    /// Declare an external function import.
    pub fn import_function(&mut self, data: FunctionSignature) -> FuncRef {
        self.func.import_function(data)
    }

    /// Returns an object with `InstBuilder` trait that allows to conveniently appending an
    /// instruction to the current `Block` being built.
    pub fn ins<'short>(&'short mut self) -> InsertBuilder<'short, FuncInstBuilder<'short, 'a>> {
        InsertBuilder::new(FuncInstBuilder::new(self, self.position))
    }

    // TODO(JW): this is not used anywhere, probably should be used.
    /// Returns a `FuncCursor` pointed at the current position ready for inserting instructions.
    ///
    /// This can be used to insert SSA code that doesn't need to access locals and that doesn't
    /// need to know about `FunctionBuilder` at all.
    pub fn cursor(&mut self) -> FuncCursor {
        self.ensure_inserted_block();
        FuncCursor::new(self.func).with_srcloc(self.srcloc).at_bottom(self.position)
    }

    /// Declare that translation of the current function is complete.
    ///
    /// This resets the state of the [`FunctionBuilderContext`] in preparation
    /// to be used for another function.
    pub fn finalize(&mut self) {
        // Check that all the `Block`s are filled and sealed.
        #[cfg(debug_assertions)]
        {
            for block in self.func_ctxt.status.keys() {
                if !self.is_pristine(block) {
                    assert!(
                        self.func_ctxt.ssa.is_sealed(block),
                        "FunctionBuilder finalized, but block {block} is not sealed",
                    );
                    assert!(
                        self.is_filled(block),
                        "FunctionBuilder finalized, but block {block} is not filled",
                    );
                }
            }
        }

        self.func.dfg.strip_alias();

        // Clear the state (but preserve the allocated buffers) in preparation
        // for translation another function.
        self.func_ctxt.clear();

        // Reset srcloc and position to initial states.
        self.srcloc = Default::default();
        // self.position = Default::default();
    }
}

/// DFG wrappers
impl FunctionBuilder<'_> {
    /// Returns the result values of an instruction.
    pub fn inst_results(&self, inst: Inst) -> &[Value] {
        self.func.dfg.inst_results(inst)
    }

    pub fn make_param(&mut self, param: Param) -> Value {
        self.func.dfg.make_param(param)
    }

    pub fn fconst(&mut self, val: Ieee64) -> Value {
        self.func.dfg.fconst(val)
    }

    pub fn iconst(&mut self, val: i32) -> Value {
        self.func.dfg.iconst(val)
    }

    pub fn sconst(&mut self, val: &str) -> Value {
        let val = self.strlit.get_or_intern(val);
        self.func.dfg.sconst(val)
    }
}

impl FunctionBuilder<'_> {
    /// Returns `true` iff the current `Block` is sealed and has no predecessors declared.
    ///
    /// The entry block of a function is never unreachable.
    pub fn is_unreachable(&self) -> bool {
        let is_entry = match self.func.layout.entry_block() {
            None => false,
            Some(entry) => self.position == entry,
        };
        !is_entry
            && self.func_ctxt.ssa.is_sealed(self.position)
            && !self.func_ctxt.ssa.has_any_predecessors(self.position)
    }

    /// Returns `true` iff no instructions have been added since the last call `switch_to_block`.
    pub fn is_pristine(&self, block: Block) -> bool {
        self.func_ctxt.status[block] == BlockStatus::Empty
    }

    /// Returns `true` iff a terminator instruction has been inserted since the last call to
    /// `switch_to_block`.
    pub fn is_filled(&self, block: Block) -> bool {
        self.func_ctxt.status[block] == BlockStatus::Filled
    }
}

/// Modify the function in ways that can be unsafe if used incorrectly.
impl FunctionBuilder<'_> {
    /// Fill the block under current position.
    ///
    /// A `Block` is 'filled' when a terminator instruction is present.
    fn fill_current_block(&mut self) {
        self.func_ctxt.status[self.position] = BlockStatus::Filled;
    }

    /// Declare the given `dst` block a successor of the current block.
    fn declare_successor(&mut self, dst: Block) {
        self.func_ctxt.ssa.declare_block_predecessor(dst, self.position);
    }
}

/// Add (potentially mutable) values to an already finished MIR function.
/// It will be available at the end of the function just like a place during building.
pub struct SSAVariableBuilder<'a> {
    ssa: SSABuilder<&'a ssa::CompleteCfg>,
}

impl<'a> SSAVariableBuilder<'a> {
    pub fn new(cfg: &'a ControlFlowGraph) -> Self {
        Self { ssa: SSABuilder::<&'a ssa::CompleteCfg>::new(cfg) }
    }

    #[must_use]
    pub fn define_at_exit(
        &mut self,
        func: &mut Function,
        init: Value,
        mut val: Value,
        inst: Inst,
    ) -> Value {
        let finished_vals = func.dfg.num_values();
        self.new_var();
        self.def_var(init, func.layout.entry_block().unwrap());
        let bb = func.layout.inst_block(inst).unwrap();
        self.def_var(val, bb);
        let exit = func.layout.exit_block().unwrap();
        val = self.use_var(func, exit);
        let res = FuncCursor::new(func).at_bottom(exit).ins().ensure_optbarrier(val);
        func.dfg.strip_alias_after(finished_vals);

        res
    }

    pub fn new_var(&mut self) {
        self.ssa.clear();
    }

    /// Defines the value of the variable at the start of a basic block
    pub fn def_var(&mut self, val: Value, bb: Block) {
        self.ssa.def_var(0u32.into(), val, bb);
    }

    /// Makes the variable available at the start of a basic block.
    /// The caller must ensure that all relevant calls to `def` have
    /// been performed
    pub fn use_var(&mut self, func: &mut Function, bb: Block) -> Value {
        self.ssa.use_var(func, 0u32.into(), bb)
    }
}
