//! See Also:
//!
//! https://docs.rs/cranelift-codegen/0.116.1/cranelift_codegen/ir/dfg/index.html

use std::fmt;

use lasso::Spur;
use typed_index_collections::TiVec;

use crate::builder::ReplaceBuilder;
use crate::entities::{Inst, Param, Tag, Value};
use crate::instructions::PhiForest;
use crate::write::write_operands;
use crate::{Block, FuncRef, FunctionSignature, Ieee64, InstructionData, Use, ValueList};

mod insts;
mod phis;
mod postorder;
mod uses;
mod values;

#[cfg(test)]
mod tests;

use insts::DfgInsts;
use values::ValueDataType;

pub use postorder::{Postorder, PostorderParts};
pub use uses::{DoubleEndedUseIter, InstUseIter, UseCursor, UseIter};
pub use values::{consts, Const, DfgValues, ValueDef};

/// A data flow graph defines all instructions and basic blocks in a function as well as
/// the data flow dependencies between them. The DFG also tracks values which can be either
/// instruction results or block parameters.
///
/// The layout of blocks in the function and of instructions in each block is recorded by the
/// `Layout` data structure which forms the other half of the function representation.
///
#[derive(Clone)]
pub struct DataFlowGraph {
    /// Instructions
    pub insts: DfgInsts,

    /// Values, constants and their uses
    pub values: DfgValues,

    /// Function signature table. These signatures are referenced by external function references.
    pub signatures: TiVec<FuncRef, FunctionSignature>,

    /// A memory pool of Phi nodes
    pub phi_forest: PhiForest,
}

impl Default for DataFlowGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl DataFlowGraph {
    /// Create a new empty `DataFlowGraph`.
    pub fn new() -> Self {
        Self {
            signatures: TiVec::new(),
            insts: DfgInsts::new(),
            values: DfgValues::new(),
            phi_forest: PhiForest::new(),
        }
    }

    /// Clear everything.
    pub fn clear(&mut self) {
        self.signatures.clear();
        self.values.clear();
        self.insts.clear();
        self.phi_forest.clear();
    }
}

/// Routines that interact with instructions.
///
/// These are just wrappers around the functions defined for `DfgInsts` for convenience
impl DataFlowGraph {
    /// Get the total number of instructions created in this function, whether they are currently
    /// inserted in the layout or not.
    pub fn num_insts(&self) -> usize {
        self.insts.num()
    }

    /// Returns `true` if the given instruction reference is valid.
    pub fn inst_is_valid(&self, inst: Inst) -> bool {
        self.insts.is_valid(inst)
    }

    /// Returns an object that displays `inst`.
    pub fn display_inst(&self, inst: Inst) -> DisplayInst {
        DisplayInst(self, inst)
    }

    /// Get the inner of a branch instruction.
    pub fn as_branch(&self, inst: Inst) -> Option<(Value, Block, Block)> {
        if let InstructionData::Branch { cond, then_dst, else_dst, .. } = self.insts[inst] {
            Some((cond, then_dst, else_dst))
        } else {
            None
        }
    }

    /// Get the function signature of a direct or indirect call instruction.
    pub fn call_signature(&self, inst: Inst) -> Option<&FunctionSignature> {
        self.func_ref(inst).map(|func_ref| &self.signatures[func_ref])
    }

    /// Get the function reference of a call instruction.
    pub fn func_ref(&self, inst: Inst) -> Option<FuncRef> {
        if let InstructionData::Call { func_ref, .. } = self.insts[inst] {
            Some(func_ref)
        } else {
            None
        }
    }

    // postorder traversal
    pub fn uses_postorder_with<'a, F: FnMut(Inst) -> bool>(
        &'a self,
        val: Value,
        parts: PostorderParts<'a>,
        descend: F,
    ) -> Postorder<'a, F> {
        let mut po = Postorder::from_parts(self, parts, descend);
        po.populate(val);
        po.traverse_successor();
        po
    }

    pub fn inst_uses_postorder<F: FnMut(Inst) -> bool>(
        &self,
        inst: Inst,
        descend: F,
    ) -> Postorder<'_, F> {
        let mut po = Postorder::new(self, descend);
        for &res in self.inst_results(inst) {
            po.populate(res);
        }
        po.traverse_successor();
        po
    }

    pub fn inst_uses_postorder_with<'a, F: FnMut(Inst) -> bool>(
        &'a self,
        inst: Inst,
        parts: PostorderParts<'a>,
        descend: F,
    ) -> Postorder<'a, F> {
        let mut po = Postorder::from_parts(self, parts, descend);
        for &res in self.inst_results(inst) {
            po.populate(res);
        }
        po.traverse_successor();
        po
    }
}

pub struct DisplayInst<'a>(&'a DataFlowGraph, Inst);

impl fmt::Display for DisplayInst<'_> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let DisplayInst(dfg, inst) = *self;

        if let Some((first, rest)) = dfg.inst_results(inst).split_first() {
            write!(f, "{}", first)?;
            for v in rest {
                write!(f, ", {}", v)?;
            }
            write!(f, " = ")?;
        }

        write!(f, "{}", dfg.insts[inst].opcode())?;
        write_operands(f, dfg, inst)
    }
}

// Dealing with results of instructions.
impl DataFlowGraph {
    /// An instruction is safe to remove if none of its results is used anywhere.
    pub fn instr_safe_to_remove(&self, inst: Inst) -> bool {
        self.insts.safe_to_remove(inst, &self.values)
    }

    pub fn has_side_effects(&self, inst: Inst, keep_branches: bool) -> bool {
        match self.insts[inst] {
            InstructionData::Branch { .. } | InstructionData::Jump { .. } => keep_branches,
            InstructionData::Call { func_ref, .. } => self.signatures[func_ref].has_side_effects,
            _ => false,
        }
    }

    /// A instruction is dead if it is safe to remove and has no side effects.
    pub fn instr_dead(&self, inst: Inst, keep_branches: bool) -> bool {
        self.insts.safe_to_remove(inst, &self.values) && !self.has_side_effects(inst, keep_branches)
    }

    /// Get all value arguments on `inst` as a slice.
    pub fn instr_args(&self, inst: Inst) -> &[Value] {
        self.insts.args(inst)
    }

    /// Get all value arguments on `inst` as a mutable slice.
    pub fn instr_args_mut(&mut self, inst: Inst) -> &mut [Value] {
        self.insts.args_mut(inst)
    }

    /// Detach the list of result values from `inst` and return it.
    ///
    /// This leaves `inst` without any result values. New result values can be created by calling
    /// `make_inst_results` or by using a `replace(inst)` builder.
    pub fn detach_results(&mut self, inst: Inst) -> ValueList {
        self.insts.detach_results(inst)
    }

    /// Clear the list of result values from `inst`.
    ///
    /// This leaves `inst` without any result values. New result values can be created by calling
    /// `make_inst_results` or by using a `replace(inst)` builder.
    pub fn clear_results(&mut self, inst: Inst) {
        self.insts.clear_results(inst)
    }

    /// Get the first result of an instruction.
    ///
    /// This function panics if the instruction doesn't have any result.
    pub fn first_result(&self, inst: Inst) -> Value {
        self.insts.first_result(inst)
    }

    /// Test if `inst` has any result values currently.
    pub fn has_results(&self, inst: Inst) -> bool {
        self.insts.has_results(inst)
    }

    /// Return all the results of an instruction.
    pub fn inst_results(&self, inst: Inst) -> &[Value] {
        self.insts.results(inst)
    }

    /// Return all the uses of an instruction.
    pub fn operands(&self, inst: Inst) -> &[Use] {
        self.insts.operands(inst)
    }

    // /// Return all the uses of an instruction.
    // pub(super) fn operands_mut(&mut self, inst: Inst) -> &mut [Use] {
    //     self.insts.operands_mut(inst)
    // }

    pub fn replace(&mut self, inst: Inst) -> ReplaceBuilder {
        ReplaceBuilder::new(self, inst)
    }
}

/// Routines that interact with `DfgValues`
impl DataFlowGraph {
    pub fn num_values(&self) -> usize {
        self.values.num()
    }

    pub fn values(&self) -> impl ExactSizeIterator<Item = Value> {
        self.values.iter()
    }

    /// Get the definition source of a value.
    #[inline]
    pub fn value_def(&self, v: Value) -> ValueDef {
        self.values.def(v)
    }

    pub fn value_valid(&self, v: Value) -> bool {
        self.values.is_valid(v)
    }

    /// A value is attached if it is an instruction result.
    /// Such value can't be attached to something else without first being detached.
    pub fn value_attached(&self, v: Value) -> bool {
        match self.values.defs[v].ty {
            ValueDataType::Inst { inst, idx, .. } => {
                Some(&v) == self.insts.results(inst).get(idx as usize)
            }
            _ => false,
        }
    }

    /// A value is dead if it is not used by any instruction in the DFG.
    pub fn value_dead(&self, val: Value) -> bool {
        self.values.is_dead(val)
    }

    pub fn get_tag(&self, val: Value) -> Option<Tag> {
        self.values.get_tag(val)
    }

    pub fn set_tag(&mut self, val: Value, tag: Option<Tag>) {
        self.values.set_tag(val, tag)
    }

    pub fn make_param(&mut self, param: Param) -> Value {
        self.values.make_param(param)
    }

    pub fn make_invalid_value(&mut self) -> Value {
        self.values.make_invalid()
    }

    pub fn iconst(&mut self, val: i32) -> Value {
        self.values.iconst(val)
    }

    pub fn f64const(&mut self, val: f64) -> Value {
        self.values.fconst(val.into())
    }

    pub fn fconst(&mut self, val: Ieee64) -> Value {
        self.values.fconst(val)
    }

    pub fn sconst(&mut self, val: Spur) -> Value {
        self.values.sconst(val)
    }
}

/// Routines that interact with `Use`s.
impl DataFlowGraph {
    /// Return an iterator over all uses of a given value
    pub fn uses(&self, value: Value) -> UseIter<'_> {
        self.values.uses(value)
    }

    /// Return a double-ended iterator over all uses of a given value
    pub fn uses_double_ended(&self, value: Value) -> DoubleEndedUseIter<'_> {
        self.values.uses_double_ended(value)
    }

    pub fn use_to_user(&self, use_: Use) -> Inst {
        self.values.use_to_user(use_)
    }

    pub fn use_to_operand(&self, use_: Use) -> (Inst, u16) {
        self.values.use_to_operand(use_)
    }

    pub fn use_to_value(&self, use_: Use) -> Value {
        self.values.use_to_value(use_, &self.insts)
    }

    pub fn use_to_value_mut(&mut self, use_: Use) -> &mut Value {
        self.values.use_to_value_mut(use_, &mut self.insts)
    }

    pub fn make_use(&mut self, val: Value, user: Inst, pos: u16) -> Use {
        self.values.make_use(val, user, pos)
    }

    pub fn attach_use(&mut self, use_: Use, val: Value) {
        self.values.attach_use(use_, val);
    }

    pub fn detach_use(&mut self, use_: Use) {
        self.values.detach_use(use_, &self.insts)
    }

    pub fn make_operand(&mut self, val: Value, inst: Inst, pos: u16) {
        let use_ = self.make_use(val, inst, pos);
        let pos_ = self.insts.uses[inst].push(use_, &mut self.insts.use_lists);
        debug_assert_eq!(pos as usize, pos_);
    }

    pub fn attach_operand(&mut self, inst: Inst, pos: u16) {
        let use_ = self.insts.uses[inst].as_slice(&self.insts.use_lists)[pos as usize];
        let val = self.insts.args(inst)[pos as usize];
        self.values.attach_use(use_, val);
    }

    pub fn detach_operand(&mut self, inst: Inst, pos: u16) {
        let use_ = self.operands(inst)[pos as usize];
        self.values.detach_use(use_, &self.insts)
    }

    pub fn set_operand_value(&mut self, val: Value, inst: Inst, pos: u16) {
        let use_ = self.insts.uses[inst].as_slice(&self.insts.use_lists)[pos as usize];
        self.set_use_value(use_, val)
    }

    pub fn set_use_value(&mut self, use_: Use, val: Value) {
        debug_assert!(!self.values.is_use_detached(use_));
        self.values.detach_use(use_, &self.insts);
        let data = self.values.uses[use_];
        self.insts.args_mut(data.user)[data.idx as usize] = val;
        self.attach_use(use_, val);
    }
}
