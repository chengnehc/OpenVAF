#![allow(non_camel_case_types)]
#![allow(non_upper_case_globals)]

//! Bindings to LLVM's C API.
//!
//! Refer to the [LLVM documentation](http://llvm.org/docs/) for more information.
//!
//! This is a vendored version of [llvm-sys](https://gitlab.com/taricorp/llvm-sys.rs)
//! adjusted to fit the needs of this project. Furthermore some improvements to llvm made in rustc
//! have been copied here.
//!
//! The buildscript from llvm-sys is replaced with the one from rustc_llvm to allow for faster
//! compile times (no regex/lazy static), cross compilation and dynamic linking.
//!
//! Furthermore, the types/functions exported here are reduced to only those actually used in OpenVAF to
//! further improve compile times

use std::marker::{PhantomData, PhantomPinned};

use libc::{c_char, c_uint, c_void};

pub mod support;

mod attributes;
mod basic_block;
mod bitcode;
mod builder;
mod context;
mod initialization;
// mod lld;
mod module;
mod pass_manager;
mod targets;
mod types;
mod values;

pub use attributes::*;
pub use basic_block::*;
pub use bitcode::*;
pub use builder::*;
pub use context::*;
pub use initialization::*;
pub use module::*;
pub use pass_manager::*;
pub use targets::*;
pub use types::*;
pub use values::*;

pub type Bool = c_uint;
pub const True: Bool = 1;
pub const False: Bool = 0;

// Opaque structure types
//
// TODO move to "extern types" when stabilized:
// - RFC: https://rust-lang.github.io/rfcs/1861-extern-types.html
// - Tracking issues: https://github.com/rust-lang/rust/issues/43467
//
// See also the FFI chapter of the "Nomicon" book:
// https://doc.rust-lang.org/nomicon/ffi.html#representing-opaque-structs

#[repr(C)]
pub struct Builder<'a> {
    _data: (),
    _marker: PhantomData<&'a mut &'a ()>,
}
#[repr(C)]
pub struct PassManager<'a> {
    _data: (),
    _marker: PhantomData<&'a mut &'a ()>,
}

#[repr(C)]
pub struct Context {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct DiagnosticInfo {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct Module {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct MemoryBuffer {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct BasicBlock {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct Type {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct Value {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct Attribute {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct PassRegistry {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct PassManagerBuilder {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct Target {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}
#[repr(C)]
pub struct TargetData {
    _data: (),
    // JW: target data should be marked with Send, as it is shared among threads.
    _marker: PhantomData<PhantomPinned>,
}
#[repr(C)]
pub struct TargetMachine {
    _data: (),
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum OptLevel {
    None = 0,
    Less = 1,
    Default = 2,
    Aggressive = 3,
}

// Only allow default CodeModel/RelocMode
// If we allow different modes we might need to change this for each module as done in rustc
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RelocMode {
    Default = 0,
    // Static = 1,
    PIC = 2,
    // DynamicNoPic = 3,
    // ROPI = 4,
    // RWPI = 5,
    // ROPI_RWPI = 6,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeModel {
    Default = 0,
    // JITDefault = 1,
    // Tiny = 2,
    // Small = 3,
    // Kernel = 4,
    // Medium = 5,
    // Large = 6,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeGenFileType {
    AssemblyFile = 0,
    ObjectFile = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeKind {
    Void = 0,
    Half = 1,
    Float = 2,
    Double = 3,
    X86_FP80 = 4,
    FP128 = 5,
    PPC_FP128 = 6,
    Label = 7,
    Integer = 8,
    Function = 9,
    Struct = 10,
    Array = 11,
    Pointer = 12,
    Vector = 13,
    Metadata = 14,
    X86_MMX = 15,
    Token = 16,
    ScalableVector = 17,
    BFloat = 18,
    X86_AMX = 19,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Linkage {
    ExternalLinkage = 0,            // Externally visible （default）
    AvailableExternallyLinkage = 1, //
    LinkOnceAnyLinkage = 2,         // Keep one copy of function when linking (inline)
    LinkOnceODRLinkage = 3,         // Same, but only replaced by something equivalent
    LinkOnceODRAutoHideLinkage = 4, //
    WeakAnyLinkage = 5,             // Keep one copy of function when linking (weak)
    WeakODRLinkage = 6,             //
    AppendingLinkage = 7,           //
    Internal = 8,                   // Rename collisions when linking (static functions)
    PrivateLinkage = 9,             // Like Internal, but omit from symbol table
    DLLImportLinkage = 10,
    DLLExportLinkage = 11,
    ExternalWeakLinkage = 12,
    GhostLinkage = 13,
    CommonLinkage = 14,
    LinkerPrivateLinkage = 15,
    LinkerPrivateWeakLinkage = 16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Default = 0,
    Hidden = 1,
    Protected = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnnamedAddr {
    /// Address of the GV is significant.
    No,
    /// Address of the GV is locally insignificant.
    Local,
    /// Address of the GV is globally insignificant.
    Global,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DLLStorageClass {
    Default = 0,
    Import = 1,
    Export = 2,
}

// LLVM CallingConv::ID. Should we wrap this?
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
#[repr(C)]
pub enum CallConv {
    CCallConv = 0,
    FastCallConv = 8,
    ColdCallConv = 9,
    // X86StdcallCallConv = 64,
    // X86FastcallCallConv = 65,
    // ArmAapcsCallConv = 67,
    // Msp430Intr = 69,
    // X86_ThisCall = 70,
    // PtxKernel = 71,
    // X86_64_SysV = 78,
    // X86_64_Win64 = 79,
    // X86_VectorCall = 80,
    // X86_Intr = 83,
    // AvrNonBlockingInterrupt = 84,
    // AvrInterrupt = 85,
    // AmdGpuKernel = 91,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntPredicate {
    IntEQ = 32,
    IntNE = 33,
    IntUGT = 34,
    IntUGE = 35,
    IntULT = 36,
    IntULE = 37,
    IntSGT = 38,
    IntSGE = 39,
    IntSLT = 40,
    IntSLE = 41,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RealPredicate {
    RealPredicateFalse = 0,
    RealOEQ = 1,
    RealOGT = 2,
    RealOGE = 3,
    RealOLT = 4,
    RealOLE = 5,
    RealONE = 6,
    RealORD = 7,
    RealUNO = 8,
    RealUEQ = 9,
    RealUGT = 10,
    RealUGE = 11,
    RealULT = 12,
    RealULE = 13,
    RealUNE = 14,
    RealPredicateTrue = 15,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd)]
pub enum DiagnosticSeverity {
    Error = 0,
    Warning = 1,
    Remark = 2,
    Note = 3,
}

pub const LLVMAttributeReturnIndex: ::libc::c_uint = 0;
pub const LLVMAttributeFunctionIndex: ::libc::c_uint = !0; // -1
/// Either LLVMAttributeReturnIndex, LLVMAttributeFunctionIndex, or a parameter
/// number from 1 to N.
pub type LLVMAttributeIndex = ::libc::c_uint;

// typedef void(* LLVMDiagnosticHandler) (LLVMDiagnosticInfoRef, void *)
pub type DiagnosticHandler = Option<extern "C" fn(diag: &DiagnosticInfo, ctx: *mut c_void)>;

// typedef void(* LLVMYieldCallback) (LLVMContextRef, void *)
pub type LLVMYieldCallback = Option<extern "C" fn(arg1: &Context, ctx: *mut c_void)>;

pub fn get_version() -> (u32, u32, u32) {
    // If RUST_CHECK is set we do not link LLVM and the version is not known, just use dummy values in that case
    (
        option_env!("LLVM_VERSION_MAJOR").map_or(14, |it| it.parse().unwrap()),
        option_env!("LLVM_VERSION_MINOR").map_or(0, |it| it.parse().unwrap()),
        option_env!("LLVM_VERSION_PATCH").map_or(6, |it| it.parse().unwrap()),
    )
}

/// Empty string, to be used where LLVM expects an instruction name, indicating
/// that the instruction is to be left unnamed (i.e. numbered, in textual IR).
// FIXME(eddyb) pass `&CStr` directly to FFI once it's a thin pointer.
pub const UNNAMED: *const c_char = c"".as_ptr() as *const c_char;
