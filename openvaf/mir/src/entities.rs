//! OpenVAF IR entity references.
//!
//! Instructions in IR need to reference other entities in the function. This can be other
//! parts of the function like basic blocks or stack slots, or it can be external entities
//! that are declared in the function preamble in the text format.
//!
//! These entity references in instruction operands are not implemented as Rust references both
//! because Rust's ownership and mutability rules make it difficult, and because 64-bit pointers
//! take up a lot of space, and we want a compact in-memory representation. Instead, entity
//! references are structs wrapping a `u32` index into a table in the `Function` main data
//! structure. There is a separate index type for each entity type, so we don't lose type safety.
//!
//! The `entities` module defines public types for the entity references along with constants
//! representing an invalid reference. We prefer to use `Option<EntityRef>` whenever possible, but
//! unfortunately that type is twice as large as the 32-bit index type on its own. Thus, compact
//! data structures use the `PackedOption<EntityRef>` representation, while function arguments and
//! return values prefer the more Rust-like `Option<EntityRef>` variant.
//!
//! The entity references all implement the `Display` trait in a way that matches the textual IR
//! format.

use std::fmt;
use stdx::{impl_debug_display, impl_from, impl_idx_from};

/// An opaque reference to a [basic block](https://en.wikipedia.org/wiki/Basic_block) in a MIR
/// function. While the order is stable, it is arbitrary and does not necessarily resemble the
/// layout order.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Block(u32);
impl_idx_from!(Block(u32));
impl_debug_display! {
    match Block {Block(i) => "block{i}";}
}

impl Block {
    /// Create a new block reference from its number. This corresponds to the `blockNN` representation.
    ///
    /// This method is for use by the parser.
    pub fn with_number(n: u32) -> Option<Self> {
        (n < u32::MAX).then_some(Self(n))
    }
}

/// An opaque reference to an SSA value.
///
/// Any `InstBuilder` instruction that has an output will also return a `Value`.
///
/// While the order is stable, it is arbitrary.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Value(u32);
impl_idx_from!(Value(u32));
impl_debug_display! {
    match Value {Value(i) => "v{i}";}
}

impl Value {
    /// Create a value from its number representation.
    /// This is the number in the `vNN` notation.
    ///
    /// This method is for use by the parser.
    pub const fn with_number(n: u32) -> Option<Self> {
        if n < u32::MAX / 2 {
            Some(Self(n))
        } else {
            None
        }
    }

    /// Create a value from its number representation.
    /// This is the number in the `vNN` notation.
    ///
    /// This method is for use by the predefined constant values.
    pub const fn with_number_(n: u32) -> Self {
        assert!(n < u32::MAX / 2);
        Self(n)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Use(u32);
impl_idx_from!(Use(u32));
impl_debug_display! {
    match Use {Use(i) => "use{i}";}
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Param(u32);
impl_idx_from!(Param(u32));
impl_debug_display! {
    match Param {Param(i) => "param{i}";}
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Tag(u32);
impl_idx_from!(Tag(u32));
impl_debug_display! {
    match Tag {Tag(i) => "tag{i}";}
}

/// An opaque reference to an instruction in a [`Function`](super::Function).
///
/// Most usage of `Inst` is internal. `Inst`ructions are returned by
/// [`InstBuilder`](super::InstBuilder) instructions that do not return a
/// [`Value`], such as control flow and trap instructions.
///
/// While the order is stable, it is arbitrary and does not necessarily resemble the layout order.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Inst(u32);
impl_idx_from!(Inst(u32));
impl_debug_display! {
    match Inst {Inst(i) => "inst{i}";}
}

/// An opaque reference to an external [`Function`](super::Function).
///
/// `FuncRef`s are used for direct function calls.
///
/// While the order is stable, it is arbitrary.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncRef(u32);
impl_idx_from!(FuncRef(u32));
impl_debug_display! {
    match FuncRef {FuncRef(i) => "inst{i}";}
}

impl FuncRef {
    /// Create a new external function reference from its number.
    ///
    /// This method is for use by the parser.
    pub fn with_number(n: u32) -> Option<Self> {
        (n < u32::MAX).then_some(Self(n))
    }
}

/// An opaque reference to a symbolic derivative unknown.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Unknown(pub u32);
impl_idx_from!(Unknown(u32));
impl_debug_display! {
    match Unknown {Unknown(i) => "unknown{i}";}
}

/// An opaque reference to any of the entities defined in this module
/// used by MIR writer.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AnyEntity {
    /// The whole function.
    Function,
    /// A basic block.
    Block(Block),
    /// An instruction.
    Inst(Inst),
    /// An SSA value.
    Value(Value),
    /// An external function.
    FuncRef(FuncRef),
}

impl_from!(Block, Inst, Value, FuncRef for AnyEntity);

impl fmt::Display for AnyEntity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Function => write!(f, "function"),
            Self::Block(r) => r.fmt(f),
            Self::Inst(r) => r.fmt(f),
            Self::Value(r) => r.fmt(f),
            Self::FuncRef(r) => r.fmt(f),
        }
    }
}

impl fmt::Debug for AnyEntity {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        (self as &dyn fmt::Display).fmt(f)
    }
}
