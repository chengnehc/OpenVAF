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
//!   constructed with correct types.
//!
//! Compared to the HIR, the MIR is completely decoupled from the AST (and HIR), which allows for much
//! faster compile times. This break comes quite naturally as the various algorithms that operate on the
//! MIR are not concerned with language-level concepts.
//!
//! Translation from HIR to MIR and mappings from MIR objects to the language-level HIR objects are
//! found in the `hir_lower` crate which is the only bridge between various MIR crates and the HIR.
//!
//! [`cranelift`]: https://github.com/bytecodealliance/wasmtime/tree/main/cranelift
//! [`cranelift_codgen`]: https://docs.rs/cranelift-codegen/latest/cranelift_codegen/index.html
//! [`llvm`]: https://github.com/llvm/llvm-project
//! [SSA]: https://en.wikipedia.org/wiki/Static_single_assignment_form

use core::fmt;
use stdx::impl_display;

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

use write::DummyResolver;

pub use cfg::ControlFlowGraph;
pub use dfg::{
    consts::*, Const, DataFlowGraph, DfgValues, InstUseIter, Postorder, UseCursor, UseIter,
    ValueDef,
};
pub use dominators::DominatorTree;
pub use entities::{AnyEntity, Block, FuncRef, Inst, Param, Unknown, Use, Value};
pub use instructions::{
    InstructionData, InstructionFormat, Opcode, PhiMap, PhiNode, ValueList, ValueListPool,
};
pub use layout::{InstIter, Layout};

/// A MIR function.
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
    /// The original source locations for each instruction. Not interpreted, only preserved.
    pub srclocs: SourceLocs,
}

impl Function {
    pub fn new() -> Function {
        Default::default()
    }

    pub fn with_name(mut self, name: String) -> Function {
        self.name = name;
        self
    }

    pub fn clear(&mut self) {
        self.dfg.clear();
        self.layout.clear();
        self.srclocs.clear();
    }

    pub fn to_debug_string(&self) -> String {
        format!("{:?}", self)
    }

    pub fn print<'a>(&'a self, interner: &'a dyn lasso::Resolver) -> DisplayFunction<'a> {
        DisplayFunction { fun: self, interner }
    }
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

impl fmt::Debug for Function {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write::write_function(fmt, self, &DummyResolver)
    }
}

pub struct DisplayFunction<'a> {
    fun: &'a Function,
    interner: &'a dyn lasso::Resolver,
}

impl fmt::Display for DisplayFunction<'_> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write::write_function(fmt, self.fun, self.interner)
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
        "{}fn %{name}({params}) -> {returns}", if *has_side_effects {""} else {"const "};
    }
}

impl Function {
    /// Adds a signature which can later be used to declare an external function import.
    pub fn import_function(&mut self, signature: FunctionSignature) -> FuncRef {
        self.dfg.signatures.push_and_get_key(signature)
    }

    /// Update the `old_pred` in the phi node with `new_pred`.
    pub fn update_phi_edges(&mut self, bb: Block, old_pred: Block, new_pred: Block) {
        for inst in self.layout.block_insts(bb) {
            let Some(PhiNode { blocks, .. }) = self.dfg.insts[inst].as_phi_mut() else { break };
            let pos = blocks.remove(old_pred, &mut self.dfg.phi_forest, &()).unwrap();
            blocks.insert(new_pred, pos, &mut self.dfg.phi_forest, &());
        }
    }

    /// Split the block into two and update phis within the block.
    ///
    /// The old block will be truncated and the new block will be starting with `before`.
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

/// An opaque 32-bit representation for source location of expressions.
/// Default value is used for those that can't be given a real source location.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SourceLoc(i32);

pub type SourceLocs = TiVec<Inst, SourceLoc>;

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
    /// Inverse the bits.
    pub fn inv(&mut self) {
        self.0 *= -1;
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

/// Known symbolic (partial) derivatives explicitly specified by user: ddx() calls.
///
/// `ddx(expr, unknown_quantity)`. According to LRM 4.5.6:
/// - *expr* is the expression for which the symbolic derivative needs to be calculated.
/// - *unknown_quantity* is the branch probe (voltage or current probe) with respect to
///   which the derivative of the expression needs to be computed. Namely. the potential
///   of a scalar net or port or the flow through a branch
///
/// ddx() calls should normally only be used for output variable evaluations.
#[derive(Debug, Clone, Default)]
pub struct KnownDerivatives {
    /// Mapping from ddx() callback ID to its positive and negative derivative unknown ID
    ///
    /// # Note
    /// Derivative `Unknown` is not to be confused with `SimUnknown`.
    pub ddx_calls: AHashMap<FuncRef, (HybridBitSet<Unknown>, HybridBitSet<Unknown>)>,

    /// Mapping from derivative unknown ID to its SSA value.
    /// A set is used for de-duplication.
    ///
    /// # Note
    /// Derivative `Unknown` is not to be confused with `SimUnknown`.
    pub unknowns: TiSet<Unknown, Value>,
}
