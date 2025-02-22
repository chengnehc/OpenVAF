//! OpenVAF MIR definition.
//!
//! The OpenVAF MIR represents the bodys of Verilog-A items as [SSA].
//! This allows very efficient implementations of the various algorithms in the backend.
//!
//! The implementation in this crate is heavily inspired by the IR in [`cranelift`] and [`llvm`].
//! However the focus of the MIR is on traditional algorithms performed at middle instead of
//! codegeneration. As a result, the implementation is simplified:
//!
//! * The MIR only represents Verilog-A, allowing for dropping support for atomics etc.
//! * The MIR does not map to actual hardware opcodes as direct codegeneration is not a goal.
//! * The MIR is untyped. All opcodes have fixed argument/return types. Instructions must be
//! constructed with correct types.
//!
//! Compared to the HIR, the MIR is completely decoupled from the AST (and HIR), which allows for much
//! faster compile times. This break comes quite naturally as the various algorithms that operate on the
//! MIR are not concerned with language-level concepts.
//!
//! Translation from HIR to MIR and mappings from MIR objects to the language-level HIR objects are
//! found in the `hir_lower` crate which is the only bridge between various MIR crates and the HIR.
//!
//! [`cranelift`]: https://github.com/bytecodealliance/wasmtime/tree/main/cranelift
//! [`llvm`]: https://github.com/llvm/llvm-project
//! [SSA]: https://en.wikipedia.org/wiki/Static_single_assignment_form
//!
//! See Also:
//!
//! https://docs.rs/cranelift-codegen/latest/cranelift_codegen/index.html

use core::fmt;
use stdx::{impl_debug, impl_display, impl_idx_from};

use ahash::AHashMap;
use bitset::HybridBitSet;
use typed_index_collections::TiVec;
use typed_indexmap::TiSet;

pub use lasso::{Interner, Spur};
pub use stdx::Ieee64;

mod dfg;
mod dominators;
mod entities;
mod instructions;
mod layout;
mod serialize;
mod validation;

pub mod builder;
pub mod cfg;
pub mod cursor;
pub mod write;

use crate::write::DummyResolver;

pub use crate::cfg::ControlFlowGraph;
pub use crate::dfg::consts::*;
pub use crate::dfg::{
    Const, DataFlowGraph, DfgValues, InstUseIter, Postorder, PostorderParts, UseCursor, UseIter,
    ValueDef,
};
pub use crate::dominators::DominatorTree;
pub use crate::entities::{AnyEntity, Block, FuncRef, Inst, Param, Use, Value};
pub use crate::instructions::{
    InstructionData, InstructionFormat, Opcode, PhiMap, PhiNode, ValueList, ValueListPool,
};
pub use crate::layout::{InstCursor, InstIter, Layout};

/// A MIR function
///
/// Functions can be cloned, but it is not a very fast operation.
/// The clone will have all the same entity numbers as the original.
#[derive(Clone, Default)]
pub struct Function {
    pub name: String,

    /// Data flow graph containing the primary definition of all instructions, blocks and values.
    pub dfg: DataFlowGraph,

    /// Layout of blocks and instructions in the function body.
    pub layout: Layout,

    /// Source locations.
    ///
    /// Track the original source location for each instruction. The source locations are not
    /// interpreted, only preserved.
    pub srclocs: SourceLocs,
}

impl AsRef<Function> for Function {
    fn as_ref(&self) -> &Function {
        self
    }
}

impl AsMut<Function> for Function {
    fn as_mut(&mut self) -> &mut Function {
        self
    }
}

impl Function {
    /// Clear all data structures in this function.
    pub fn clear(&mut self) {
        self.dfg.clear();
        self.layout.clear();
        self.srclocs.clear();
    }

    pub fn new() -> Function {
        Self {
            name: String::new(),
            dfg: DataFlowGraph::new(),
            layout: Layout::new(),
            srclocs: TiVec::new(),
        }
    }

    pub fn with_name(name: String) -> Function {
        let mut func = Function::new();
        func.name = name;
        func
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FunctionSignature {
    pub name: String,
    pub params: u16,
    pub returns: u16,
    pub has_side_effects: bool,
}
impl_display! {
    match FunctionSignature{
        FunctionSignature{name, params, returns, has_side_effects} =>
        "{}fn %{}({}) -> {}", if *has_side_effects {""} else {"const "}, name, params, returns;
    }
}

impl Function {
    /// Adds a signature which can later be used to declare an external function import.
    pub fn import_function(&mut self, signature: FunctionSignature) -> FuncRef {
        self.dfg.signatures.push_and_get_key(signature)
    }

    pub fn update_phi_edges(&mut self, bb: Block, old_pred: Block, new_pred: Block) {
        for inst in self.layout.block_insts(bb) {
            if let InstructionData::PhiNode(PhiNode { ref mut blocks, .. }) = self.dfg.insts[inst] {
                let pos = blocks.remove(old_pred, &mut self.dfg.phi_forest, &()).unwrap();
                blocks.insert(new_pred, pos, &mut self.dfg.phi_forest, &());
            } else {
                break;
            }
        }
    }

    /// Split the block containing `before` into two.
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
    pub fn split_block(&mut self, new_block: Block, before: Inst) {
        let old_block = self.layout.inst_block(before).unwrap();
        self.layout.split_block(new_block, before);
        if let Some(term) = self.layout.block_terminator(new_block) {
            match self.dfg.insts[term] {
                InstructionData::Jump { destination } => {
                    self.update_phi_edges(destination, old_block, new_block)
                }
                InstructionData::Branch { then_dst, else_dst, .. } => {
                    self.update_phi_edges(then_dst, old_block, new_block);
                    self.update_phi_edges(else_dst, old_block, new_block);
                }
                _ => (),
            }
        }
    }
    /* JW: not used.
        pub fn remove_opt_barriers(&mut self) {
            for inst in self.dfg.insts.iter() {
                if let InstructionData::Unary { opcode: Opcode::OptBarrier, arg } = self.dfg.insts[inst]
                {
                    if self.layout.inst_block(inst).is_some() {
                        let res = self.dfg.first_result(inst);
                        self.dfg.replace_uses(res, arg);
                        self.layout.remove_inst(inst)
                    }
                }
            }
        }
    */
}

// Print a MIR function to text.
pub struct PrintableFunction<'a> {
    fun: &'a Function,
    resolver: &'a dyn lasso::Resolver,
}

impl fmt::Display for PrintableFunction<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write::write_function(fmt, self.fun, self.resolver)
    }
}

impl Function {
    pub fn print<'a>(&'a self, resolver: &'a dyn lasso::Resolver) -> PrintableFunction<'a> {
        PrintableFunction { fun: self, resolver }
    }

    pub fn to_debug_string(&self) -> String {
        format!("{:?}", self)
    }
}

impl fmt::Debug for Function {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write::write_function(fmt, self, &DummyResolver)
    }
}

/// Source locations for instructions.
pub type SourceLocs = TiVec<Inst, SourceLoc>;

/// A source location.
///
/// This is an opaque 32-bit number attached to each IR instruction.
///
// JW: cranelift IR uses the all-ones bit pattern `!0` as default, which is not the case here.
///
/// Default value is used for instructions that can't be given a real source location.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SourceLoc(pub i32); // consider making the inner representation private

impl SourceLoc {
    /// Create a new source location with the given bits.
    pub fn new(bits: i32) -> Self {
        Self(bits)
    }

    /// Is this the default source location?
    pub fn is_default(self) -> bool {
        self == Default::default()
    }

    /// Read the bits of this source location.
    pub fn bits(self) -> i32 {
        self.0
    }
}

impl fmt::Display for SourceLoc {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.is_default() {
            write!(f, "@-")
        } else {
            write!(f, "@{:04x}", self.0)
        }
    }
}

/// An equation unknown.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Unknown(pub u32);
impl_idx_from!(Unknown(u32));
impl_debug!(match Unknown{Unknown(raw) => "unknown{}",raw;});

#[derive(Debug, Clone, Default)]
pub struct KnownDerivatives {
    pub unknowns: TiSet<Unknown, Value>,
    pub ddx_calls: AHashMap<FuncRef, (HybridBitSet<Unknown>, HybridBitSet<Unknown>)>,
    // pub standin_calls: AHashMap<FuncRef, u32>,
}

// TODO(JW) what is the purpose of this func?
pub fn strip_optbarrier(func: impl AsRef<Function>, mut val: Value) -> Value {
    let func = func.as_ref();
    while let Some(inst) = func.dfg.value_def(val).inst() {
        if let InstructionData::Unary { opcode: Opcode::OptBarrier, arg } = func.dfg.insts[inst] {
            val = arg;
        } else {
            break;
        }
    }
    val
}
