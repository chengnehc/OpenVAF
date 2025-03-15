//! Definitions for instruction formats, opcodes, and the in-memory representation of IR instructions.
//!
//! A large part of this module is auto-generated.

use std::hash::Hash;
use std::{fmt, slice};

use crate::entities::{Block, FuncRef, Use, Value};

#[rustfmt::skip]
mod generated;
pub use generated::*;

/// Some instructions use an external list of argument values to fit in the very
/// compact 16-byte structure `InstructionData`.
pub type ValueList = list_pool::ListHandle<Value>;
pub type UseList = list_pool::ListHandle<Use>;

/// Memory pool
pub type ValueListPool = list_pool::ListPool<Value>;
pub type UseListPool = list_pool::ListPool<Use>;

#[derive(Clone, Debug)]
pub enum InstructionData {
    Unary { opcode: Opcode, arg: Value },
    Binary { opcode: Opcode, args: [Value; 2] },
    Branch { cond: Value, then_dst: Block, else_dst: Block, loop_entry: bool },
    Jump { destination: Block },
    Call { func_ref: FuncRef, args: ValueList },
    PhiNode(PhiNode),
}

impl From<PhiNode> for InstructionData {
    fn from(node: PhiNode) -> Self {
        InstructionData::PhiNode(node)
    }
}

#[test]
fn instruction_data_size() {
    assert_eq!(std::mem::size_of::<InstructionData>(), 16)
}

use InstructionData::*;

impl InstructionData {
    pub fn is_terminator(&self) -> bool {
        matches!(self, InstructionData::Branch { .. } | InstructionData::Jump { .. })
    }

    pub fn is_phi(&self) -> bool {
        matches!(self, InstructionData::PhiNode(_))
    }

    pub fn unwrap_phi(&self) -> &PhiNode {
        let InstructionData::PhiNode(node) = self else { unreachable!() };
        node
    }

    pub fn unwrap_phi_mut(&mut self) -> &mut PhiNode {
        let InstructionData::PhiNode(node) = self else { unreachable!() };
        node
    }

    /// Get references to the value arguments to this instruction.
    pub fn arguments<'a>(&'a self, pool: &'a ValueListPool) -> &'a [Value] {
        match self {
            Unary { arg, .. } | Branch { cond: arg, .. } => slice::from_ref(arg),
            Binary { args, .. } => args,
            Call { args, .. } | PhiNode(PhiNode { args, .. }) => args.as_slice(pool),
            Jump { .. } => &[],
        }
    }

    /// Get mutable references to the value arguments to this instruction.
    ///
    /// # Note
    /// It is up to the caller to ensure that uses are updates as appropriate
    pub fn arguments_mut<'a>(&'a mut self, pool: &'a mut ValueListPool) -> &'a mut [Value] {
        match self {
            Unary { arg, .. } | Branch { cond: arg, .. } => slice::from_mut(arg),
            Binary { args, .. } => &mut *args,
            Call { args, .. } | PhiNode(PhiNode { args, .. }) => args.as_mut_slice(pool),
            Jump { .. } => &mut [],
        }
    }

    /// Get the opcode of this instruction.
    pub fn opcode(&self) -> Opcode {
        match self {
            Unary { opcode: op, .. } | Binary { opcode: op, .. } => *op,
            Branch { .. } => Opcode::Br,
            Jump { .. } => Opcode::Jmp,
            Call { .. } => Opcode::Call,
            PhiNode(PhiNode { .. }) => Opcode::Phi,
        }
    }

    pub fn eq(&self, other: &Self, val_pool: &ValueListPool, phi_forest: &PhiForest) -> bool {
        match (self, other) {
            (Unary { opcode: l_op, arg: l_arg }, Unary { opcode: r_op, arg: r_arg }) => {
                l_op == r_op && l_arg == r_arg
            }
            (Binary { opcode: l_op, args: l_args }, Binary { opcode: r_op, args: r_args }) => {
                l_op == r_op && l_args == r_args
            }
            (
                Branch {
                    cond: l_cond,
                    then_dst: l_then_dst,
                    else_dst: l_else_dst,
                    loop_entry: l_loop_entry,
                },
                Branch {
                    cond: r_cond,
                    then_dst: r_then_dst,
                    else_dst: r_else_dst,
                    loop_entry: r_loop_entry,
                },
            ) => {
                l_cond == r_cond
                    && l_then_dst == r_then_dst
                    && l_else_dst == r_else_dst
                    && l_loop_entry == r_loop_entry
            }
            (Jump { destination: l }, Jump { destination: r }) => l == r,
            (
                Call { func_ref: l_func_ref, args: l_args },
                Call { func_ref: r_func_ref, args: r_args },
            ) => l_func_ref == r_func_ref && l_args.as_slice(val_pool) == r_args.as_slice(val_pool),

            (Self::PhiNode(lnode), Self::PhiNode(rnode)) => lnode.eq(rnode, val_pool, phi_forest),

            _ => false,
        }
    }

    pub fn hash<H: std::hash::Hasher>(
        &self,
        state: &mut H,
        val_pool: &ValueListPool,
        phi_forest: &PhiForest,
    ) {
        core::mem::discriminant(self).hash(state);
        match self {
            Unary { opcode: op, arg } => {
                op.hash(state);
                arg.hash(state);
            }
            Binary { opcode: op, args } => {
                op.hash(state);
                args.hash(state);
            }
            Branch { cond, then_dst, else_dst, loop_entry } => {
                cond.hash(state);
                then_dst.hash(state);
                else_dst.hash(state);
                loop_entry.hash(state);
            }
            Jump { destination } => destination.hash(state),
            Call { func_ref, args } => {
                func_ref.hash(state);
                args.as_slice(val_pool).hash(state);
            }
            PhiNode(node) => node.hash(state, val_pool, phi_forest),
        }
    }

    #[inline]
    pub fn to_pool<'a>(
        &self,
        val_pool: &'a ValueListPool,
        phi_forest: &'a PhiForest,
        dst_val_pool: &'a mut ValueListPool,
        dst_phi_forest: &'a mut PhiForest,
    ) -> Self {
        let mut res = self.clone();
        match &mut res {
            PhiNode(phi) => *phi = phi.to_pool(val_pool, phi_forest, dst_val_pool, dst_phi_forest),
            Call { args, .. } => *args = args.to_pool(val_pool, dst_val_pool),
            _ => (),
        }
        res
    }
}

impl Opcode {
    #[inline]
    pub fn is_branch(self) -> bool {
        matches!(self, Opcode::Jmp | Opcode::Br)
    }

    #[inline]
    pub fn is_call(self) -> bool {
        matches!(self, Opcode::Call)
    }

    #[inline]
    pub fn is_commutative(self) -> bool {
        matches!(
            self,
            Opcode::Fmul
                | Opcode::Fadd
                | Opcode::Iand
                | Opcode::Ixor
                | Opcode::Ior
                | Opcode::Iadd
                | Opcode::Imul
                | Opcode::Ieq
                | Opcode::Feq
                | Opcode::Beq
                | Opcode::Seq
                | Opcode::Ine
                | Opcode::Fne
                | Opcode::Bne
                | Opcode::Sne
        )
    }

    #[inline]
    pub const fn constraints(self) -> OpcodeConstraints {
        OPCODE_CONSTRAINTS[self as usize]
    }

    #[inline]
    pub const fn format(self) -> InstructionFormat {
        OPCODE_FORMAT[self as usize]
    }

    #[inline]
    pub const fn name(self) -> &'static str {
        OPCODE_NAMES[self as usize]
    }
}

impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl fmt::Debug for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Value type constraints for a given opcode.
///
/// The `InstructionFormat` determines the constraints on most operands, but `Value` operands and
/// results are not determined by the format. Every `Opcode` has an associated
/// `OpcodeConstraints` object that provides the missing details.
#[derive(Clone, Copy)]
pub struct OpcodeConstraints {
    /// Flags for this opcode encoded as a bit field:
    ///
    /// Bits 0-2:
    ///     Number of fixed result values. This does not include `variable_args` results as are
    ///     produced by call instructions.
    ///
    /// Bits 3-5:
    ///     Number of fixed value arguments. The minimum required number of value operands.
    flags: u8,
}

impl OpcodeConstraints {
    const fn new(arg_cnt: u8, ret_cnt: u8) -> OpcodeConstraints {
        OpcodeConstraints { flags: (arg_cnt << 3) | ret_cnt }
    }

    /// Get the number of *fixed* result values produced by this opcode.
    ///
    /// This does not include `variable_args` produced by calls.
    pub fn num_fixed_results(self) -> usize {
        (self.flags & 0x7) as usize
    }

    /// Get the number of *fixed* input values required by this opcode.
    ///
    /// This does not include `variable_args` arguments on call and branch instructions.
    ///
    /// The number of fixed input values is usually implied by the instruction format, but
    /// instruction formats that use a `ValueList` put both fixed and variable arguments in the
    /// list. This method returns the *minimum* number of values required in the value list.
    pub fn num_fixed_value_arguments(self) -> usize {
        ((self.flags >> 3) & 0x7) as usize
    }
}

// PHI (Φ) nodes are required when a variable can be assigned a different value based on the path of control flow.

/// Mapping from incoming blocks to related value positions in `ValueList`
pub type PhiMap = bforest::Map<Block, u32>;
/// Memory pool for `PhiMap`s
pub type PhiForest = bforest::MapForest<Block, u32>;

/// A PHI instruction, for instance:
/// ```mir
/// phi [v4, block5], [v5, block6]
/// ```
#[derive(Clone, Debug)]
pub struct PhiNode {
    pub args: ValueList,
    pub blocks: PhiMap,
}

#[derive(Clone, Copy)]
pub struct PhiEdges<'a> {
    args: &'a [Value],
    iter: bforest::MapIter<'a, Block, u32>,
}

impl Iterator for PhiEdges<'_> {
    type Item = (Block, Value);

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(move |(block, pos)| (block, self.args[pos as usize]))
    }
}

impl PhiNode {
    #[inline]
    pub fn edges<'a>(
        &self,
        val_pool: &'a ValueListPool,
        phi_forest: &'a PhiForest,
    ) -> PhiEdges<'a> {
        let args = self.args.as_slice(val_pool);
        PhiEdges { args, iter: self.blocks.iter(phi_forest) }
    }

    /// The corresponding value of `block` in the phi instruction
    #[inline]
    pub fn edge_val(
        &self,
        block: Block,
        val_pool: &ValueListPool,
        phi_forest: &PhiForest,
    ) -> Option<Value> {
        let pos = self.edge_operand(block, phi_forest)?;
        Some(self.args.as_slice(val_pool)[pos as usize])
    }

    #[inline]
    fn edge_operand(&self, block: Block, phi_forest: &PhiForest) -> Option<u32> {
        self.blocks.get(block, phi_forest, &())
    }

    #[inline]
    pub fn to_pool<'a>(
        &self,
        val_pool: &'a ValueListPool,
        phi_forest: &'a PhiForest,
        dst_val_pool: &'a mut ValueListPool,
        dst_phi_forest: &'a mut PhiForest,
    ) -> Self {
        let args = self.args.to_pool(val_pool, dst_val_pool);
        let mut blocks = PhiMap::new();
        blocks.insert_sorted_iter(self.blocks.iter(phi_forest), dst_phi_forest, &(), |_, i| i);
        Self { args, blocks }
    }

    #[inline]
    pub fn eq(&self, other: &Self, val_pool: &ValueListPool, phi_forest: &PhiForest) -> bool {
        let l_edges = self.edges(val_pool, phi_forest);
        let r_edges = other.edges(val_pool, phi_forest);
        l_edges.eq(r_edges)
    }

    #[inline]
    pub fn hash<H: std::hash::Hasher>(
        &self,
        state: &mut H,
        val_pool: &ValueListPool,
        phi_forest: &PhiForest,
    ) {
        for (block, val) in self.edges(val_pool, phi_forest) {
            block.hash(state);
            val.hash(state)
        }
    }
}
