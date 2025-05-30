//! LLVM IR Builder

use std::ffi::c_uint;
use std::slice;

use arrayvec::ArrayVec;
use llvm::UNNAMED;
use mir::{ControlFlowGraph, Opcode, ValueDef, F_ZERO, ZERO};
use typed_index_collections::TiVec;

use crate::callbacks::CallbackFun;
use crate::CodegenCx;

#[derive(Clone)]
pub struct MemLoc<'ll> {
    /// The base pointer of the aggregate type
    pub base_ptr: &'ll llvm::Value,
    /// The aggregate type (array/struct) in which the `MemLoc` resides
    pub ty: &'ll llvm::Type,
    /// The type of this MemLoc element
    pub elem_ty: &'ll llvm::Type,
    /// Each layer of indirection requires an index
    pub indices: Box<[&'ll llvm::Value]>,
}

impl<'ll> MemLoc<'ll> {
    pub fn struct_gep(
        struct_ptr: &'ll llvm::Value,
        ty: &'ll llvm::Type,
        elem_ty: &'ll llvm::Type,
        idx: u32,
        cx: &CodegenCx<'_, 'll>,
    ) -> MemLoc<'ll> {
        MemLoc {
            base_ptr: struct_ptr,
            ty,
            elem_ty,
            indices: Box::new([cx.const_unsigned_int(0), cx.const_unsigned_int(idx)]),
        }
    }

    pub fn read(&self, llbuilder: &llvm::Builder<'ll>) -> &'ll llvm::Value {
        unsafe { self.read_with_ptr(llbuilder, self.base_ptr) }
    }

    /// # Safety
    pub unsafe fn read_with_ptr(
        &self,
        llbuilder: &llvm::Builder<'ll>,
        ptr: &'ll llvm::Value,
    ) -> &'ll llvm::Value {
        unsafe {
            let ptr = self.to_ptr_from(llbuilder, ptr);
            llvm::LLVMBuildLoad2(llbuilder, self.elem_ty, ptr, UNNAMED)
        }
    }

    unsafe fn to_ptr_from(
        &self,
        llbuilder: &llvm::Builder<'ll>,
        base_ptr: &'ll llvm::Value,
    ) -> &'ll llvm::Value {
        if self.indices.is_empty() {
            base_ptr
        } else {
            unsafe {
                llvm::LLVMBuildGEP2(
                    llbuilder,
                    self.ty,
                    base_ptr,
                    self.indices.as_ptr(),
                    self.indices.len() as u32,
                    UNNAMED,
                )
            }
        }
    }
}

#[derive(Clone)]
pub enum BuilderVal<'ll> {
    Undef,
    Eager(&'ll llvm::Value),
    Load(Box<MemLoc<'ll>>),
    Call(Box<CallbackFun<'ll>>),
}

impl<'ll> From<MemLoc<'ll>> for BuilderVal<'ll> {
    fn from(loc: MemLoc<'ll>) -> Self {
        BuilderVal::Load(Box::new(loc))
    }
}

impl<'ll> From<&'ll llvm::Value> for BuilderVal<'ll> {
    fn from(val: &'ll llvm::Value) -> Self {
        BuilderVal::Eager(val)
    }
}

impl<'ll> BuilderVal<'ll> {
    /// Get the underlying LLVM value
    pub fn get(&self, builder: &Builder<'_, '_, 'll>) -> &'ll llvm::Value {
        match self {
            BuilderVal::Undef => unreachable!("attempted to read undefined value"),
            BuilderVal::Eager(val) => val,
            BuilderVal::Load(loc) => loc.read(builder.llbuilder),
            BuilderVal::Call(call) => unsafe { builder.call(call.fun_ty, call.fun, &call.state) },
        }
    }

    /// Get the type of the underlying LLVM value
    pub fn get_ty(&self, builder: &Builder<'_, '_, 'll>) -> Option<&'ll llvm::Type> {
        let ty = match self {
            BuilderVal::Undef => return None,
            BuilderVal::Eager(val) => builder.cx.ty_of(val),
            BuilderVal::Load(loc) => loc.elem_ty,
            BuilderVal::Call(call) => unsafe { llvm::LLVMGetReturnType(call.fun_ty) },
        };
        Some(ty)
    }
}

// A `mir_llvm::Builder` must have an associative `llfunc`
#[must_use]
pub struct Builder<'a, 'cx, 'll> {
    pub cx: &'a CodegenCx<'cx, 'll>,
    pub llbuilder: &'a mut llvm::Builder<'ll>,
    pub llfunc: &'ll llvm::Value,
    pub mir: &'a mir::Function,
    pub blocks: TiVec<mir::Block, Option<&'ll llvm::BasicBlock>>,
    pub values: TiVec<mir::Value, BuilderVal<'ll>>,
    pub params: TiVec<mir::Param, BuilderVal<'ll>>,
    pub callbacks: TiVec<mir::FuncRef, Option<CallbackFun<'ll>>>,
    pub unfinished_phis: Vec<(mir::PhiNode, &'ll llvm::Value)>,
    // pub prepend_pos: &'ll llvm::BasicBlock,
}

impl Drop for Builder<'_, '_, '_> {
    fn drop(&mut self) {
        unsafe { llvm::LLVMDisposeBuilder(&mut *(self.llbuilder as *mut _)) }
    }
}

#[derive(Clone, Copy)]
pub enum FastMathMode {
    Full,
    Partial,
    Disabled,
}

impl<'a, 'cx, 'll> Builder<'a, 'cx, 'll> {
    pub fn new(
        cx: &'a CodegenCx<'cx, 'll>,
        mir: &'a mir::Function,
        llfunc: &'ll llvm::Value,
    ) -> Self {
        let entry = unsafe { llvm::LLVMAppendBasicBlockInContext(cx.llcx, llfunc, UNNAMED) };
        let llbuilder = unsafe { llvm::LLVMCreateBuilderInContext(cx.llcx) };
        let mut blocks: TiVec<_, _> = vec![None; mir.layout.num_blocks()].into();
        for blk in mir.layout.blocks() {
            blocks[blk] =
                unsafe { Some(llvm::LLVMAppendBasicBlockInContext(cx.llcx, llfunc, UNNAMED)) };
        }
        unsafe { llvm::LLVMPositionBuilderAtEnd(llbuilder, entry) };

        Builder {
            llbuilder,
            llfunc,
            cx,
            mir,
            blocks,
            values: vec![BuilderVal::Undef; mir.dfg.num_values()].into(),
            params: Default::default(),
            callbacks: Default::default(),
            unfinished_phis: Vec::new(),
            // prepend_pos: entry,
        }
    }
}

impl<'ll> Builder<'_, '_, 'll> {
    pub fn build_consts(&mut self) {
        for val in self.mir.dfg.values() {
            match self.mir.dfg.value_def(val) {
                ValueDef::Result(_, _) | ValueDef::Invalid => (),
                ValueDef::Param(param) => self.values[val] = self.params[param].clone(),
                ValueDef::Const(const_val) => {
                    self.values[val] = self.cx.const_val(&const_val).into();
                }
            }
        }
    }

    /// # Safety
    /// Must not be called if any block already contain any non-phi instruction
    /// (e.g. must not be called twice)
    pub unsafe fn build_func(&mut self) {
        let entry = self.mir.layout.entry_block().unwrap();
        unsafe { llvm::LLVMBuildBr(self.llbuilder, self.blocks[entry].unwrap()) };

        let cfg = ControlFlowGraph::with_function(self.mir);
        let po: Vec<_> = cfg.postorder(self.mir).collect();
        drop(cfg);
        for bb in po.into_iter().rev() {
            unsafe { self.build_bb(bb) }
        }

        for (phi, llval) in self.unfinished_phis.iter() {
            let (blocks, vals): (Vec<_>, Vec<_>) = self
                .mir
                .dfg
                .phi_edges(phi)
                .map(|(bb, val)| {
                    self.select_bb_before_terminator(bb);
                    (self.blocks[bb].unwrap(), self.values[val].get(self))
                })
                .unzip();

            unsafe {
                llvm::LLVMAddIncoming(llval, vals.as_ptr(), blocks.as_ptr(), vals.len() as c_uint)
            };
        }
        self.unfinished_phis.clear();
    }

    pub fn select_bb(&self, bb: mir::Block) {
        unsafe { llvm::LLVMPositionBuilderAtEnd(self.llbuilder, self.blocks[bb].unwrap()) }
    }

    /// Position the builder before the terminator instruction of specified block.
    pub fn select_bb_before_terminator(&self, bb: mir::Block) {
        let bb = self.blocks[bb].unwrap();
        unsafe {
            let inst = llvm::LLVMGetLastInstruction(bb);
            llvm::LLVMPositionBuilder(self.llbuilder, bb, inst);
        };
    }

    /// # Safety
    /// * Must not be called if any non phi instruction has already been build for `bb`.
    /// * Must not be called twice for the same block.
    pub unsafe fn build_bb(&mut self, bb: mir::Block) {
        self.select_bb(bb);
        for inst in self.mir.layout.block_insts(bb) {
            // TODO(JW): why using inversed srcloc (i32) to indicate fast math mode?
            let fast_math = self.mir.srclocs.get(inst).is_some_and(|loc| loc.bits() < 0);
            unsafe {
                self.build_inst(
                    inst,
                    if fast_math { FastMathMode::Partial } else { FastMathMode::Disabled },
                )
            }
        }
    }

    /// # Safety
    /// * Must only be called when after the builder has been positioned.
    /// * No Phis may be constructed for the current block after this function has been called.
    /// * Must not be called when the builder has selected a block that already contains a terminator.
    pub unsafe fn build_inst(&mut self, inst: mir::Inst, fast_math_mode: FastMathMode) {
        let (opcode, args) = match self.mir.dfg.insts[inst] {
            mir::InstructionData::Unary { opcode, ref arg } => (opcode, slice::from_ref(arg)),
            mir::InstructionData::Binary { opcode, ref args } => (opcode, args.as_slice()),
            mir::InstructionData::Jump { destination } => {
                unsafe { llvm::LLVMBuildBr(self.llbuilder, self.blocks[destination].unwrap()) };
                return;
            }
            mir::InstructionData::Branch { cond, then_dst, else_dst, .. } => {
                unsafe {
                    llvm::LLVMBuildCondBr(
                        self.llbuilder,
                        self.values[cond].get(self),
                        self.blocks[then_dst].unwrap(),
                        self.blocks[else_dst].unwrap(),
                    )
                };
                return;
            }
            mir::InstructionData::PhiNode(ref phi) => {
                // TODO does this always produce a valid value?
                let ty = self
                    .mir
                    .dfg
                    .phi_edges(phi)
                    .find_map(|(_, val)| self.values[val].get_ty(self))
                    .unwrap();
                let llval = unsafe { llvm::LLVMBuildPhi(self.llbuilder, ty, UNNAMED) };
                self.unfinished_phis.push((phi.clone(), llval));
                let val = self.mir.dfg.first_result(inst);
                self.values[val] = llval.into();
                return;
            }
            mir::InstructionData::Call { func_ref, ref args } => {
                let Some(callback) = self.callbacks[func_ref].as_ref() else { return }; // assume noop

                let args = args.as_slice(&self.mir.dfg.insts.value_lists);
                let args = args.iter().map(|operand| self.values[*operand].get(self));

                if callback.num_state != 0 {
                    let args: Vec<_> = args.collect();
                    let num_iter = callback.state.len() as u32 / callback.num_state;
                    for i in 0..num_iter {
                        let start = (i * callback.num_state) as usize;
                        let end = ((i + 1) * callback.num_state) as usize;
                        let operands: Vec<_> = callback.state[start..end]
                            .iter()
                            .copied()
                            .chain(args.iter().copied())
                            .collect();
                        unsafe { self.call(callback.fun_ty, callback.fun, &operands) };
                        debug_assert!(self.mir.dfg.inst_results(inst).is_empty());
                    }
                } else {
                    let operands: Vec<_> = callback.state.iter().copied().chain(args).collect();
                    let res = unsafe { self.call(callback.fun_ty, callback.fun, &operands) };
                    let inst_res = self.mir.dfg.inst_results(inst);

                    match inst_res {
                        [] => (),
                        [val] => self.values[*val] = res.into(),
                        vals => {
                            for (i, val) in vals.iter().enumerate() {
                                let res = unsafe {
                                    llvm::LLVMBuildExtractValue(
                                        self.llbuilder,
                                        res,
                                        i as u32,
                                        UNNAMED,
                                    )
                                };
                                self.values[*val] = res.into();
                            }
                        }
                    }
                }
                return;
            }
        };

        let val = match opcode {
            Opcode::Inot | Opcode::Bnot => {
                let arg = self.values[args[0]].get(self);
                unsafe { llvm::LLVMBuildNot(self.llbuilder, arg, UNNAMED) }
            }
            Opcode::Ineg => {
                let arg = self.values[args[0]].get(self);
                unsafe { llvm::LLVMBuildNeg(self.llbuilder, arg, UNNAMED) }
            }
            Opcode::Fneg => {
                let arg = self.values[args[0]].get(self);
                unsafe { llvm::LLVMBuildFNeg(self.llbuilder, arg, UNNAMED) }
            }
            Opcode::IFcast => {
                let arg = self.values[args[0]].get(self);
                unsafe { llvm::LLVMBuildSIToFP(self.llbuilder, arg, self.cx.ty_double(), UNNAMED) }
            }
            Opcode::BFcast => {
                let arg = self.values[args[0]].get(self);
                unsafe { llvm::LLVMBuildUIToFP(self.llbuilder, arg, self.cx.ty_double(), UNNAMED) }
            }
            Opcode::BIcast => {
                let arg = self.values[args[0]].get(self);
                unsafe {
                    llvm::LLVMBuildIntCast2(
                        self.llbuilder,
                        arg,
                        self.cx.ty_int(),
                        llvm::False,
                        UNNAMED,
                    )
                }
            }
            Opcode::FIcast => self.intrinsic(args, "llvm.lround.i32.f64"),
            Opcode::IBcast => unsafe {
                self.build_int_cmp(&[args[0], ZERO], llvm::IntPredicate::IntNE)
            },
            Opcode::FBcast => unsafe {
                self.build_real_cmp(&[args[0], F_ZERO], llvm::RealPredicate::RealONE)
            },
            Opcode::Iadd => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildAdd(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Isub => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildSub(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Imul => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildMul(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Idiv => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildSDiv(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Irem => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildSRem(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Ishl => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildShl(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Ishr => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildLShr(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Ixor => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildXor(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Iand => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildAnd(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Ior => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildOr(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Fadd => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildFAdd(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Fsub => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildFSub(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Fmul => {
                if matches!(self.values[args[0]], BuilderVal::Undef) {
                    panic!(
                        "{} {}",
                        self.mir.dfg.display_inst(inst),
                        self.mir.layout.inst_block(inst).unwrap()
                    );
                }
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildFMul(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Fdiv => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildFDiv(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Frem => {
                let lhs = self.values[args[0]].get(self);
                let rhs = self.values[args[1]].get(self);
                unsafe { llvm::LLVMBuildFRem(self.llbuilder, lhs, rhs, UNNAMED) }
            }
            Opcode::Ilt => unsafe { self.build_int_cmp(args, llvm::IntPredicate::IntSLT) },
            Opcode::Igt => unsafe { self.build_int_cmp(args, llvm::IntPredicate::IntSGT) },
            Opcode::Ile => unsafe { self.build_int_cmp(args, llvm::IntPredicate::IntSLE) },
            Opcode::Ige => unsafe { self.build_int_cmp(args, llvm::IntPredicate::IntSGE) },
            Opcode::Ieq | Opcode::Beq => unsafe {
                self.build_int_cmp(args, llvm::IntPredicate::IntEQ)
            },
            Opcode::Ine | Opcode::Bne => unsafe {
                self.build_int_cmp(args, llvm::IntPredicate::IntNE)
            },
            Opcode::Flt => unsafe { self.build_real_cmp(args, llvm::RealPredicate::RealOLT) },
            Opcode::Fgt => unsafe { self.build_real_cmp(args, llvm::RealPredicate::RealOGT) },
            Opcode::Fle => unsafe { self.build_real_cmp(args, llvm::RealPredicate::RealOLE) },
            Opcode::Fge => unsafe { self.build_real_cmp(args, llvm::RealPredicate::RealOGE) },
            Opcode::Feq => unsafe { self.build_real_cmp(args, llvm::RealPredicate::RealOEQ) },
            Opcode::Fne => unsafe { self.build_real_cmp(args, llvm::RealPredicate::RealONE) },
            Opcode::Seq => unsafe { self.strcmp(args, false) },
            Opcode::Sne => unsafe { self.strcmp(args, true) },
            Opcode::Sqrt => self.intrinsic(args, "llvm.sqrt.f64"),
            Opcode::Exp => self.intrinsic(args, "llvm.exp.f64"),
            Opcode::Ln => self.intrinsic(args, "llvm.log.f64"),
            Opcode::Log => self.intrinsic(args, "llvm.log10.f64"),
            Opcode::Clog2 => {
                let leading_zeros = self.intrinsic(&[args[0], true.into()], "llvm.ctlz");
                let total_bits = self.cx.const_int(32);
                unsafe { llvm::LLVMBuildSub(self.llbuilder, total_bits, leading_zeros, UNNAMED) }
            }
            Opcode::Floor => self.intrinsic(args, "llvm.floor.f64"),
            Opcode::Ceil => self.intrinsic(args, "llvm.ceil.f64"),
            Opcode::Sin => self.intrinsic(args, "llvm.sin.f64"),
            Opcode::Cos => self.intrinsic(args, "llvm.cos.f64"),
            Opcode::Tan => self.intrinsic(args, "tan"),
            Opcode::Hypot => self.intrinsic(args, "hypot"),
            Opcode::Asin => self.intrinsic(args, "asin"),
            Opcode::Acos => self.intrinsic(args, "acos"),
            Opcode::Atan => self.intrinsic(args, "atan"),
            Opcode::Atan2 => self.intrinsic(args, "atan2"),
            Opcode::Sinh => self.intrinsic(args, "sinh"),
            Opcode::Cosh => self.intrinsic(args, "cosh"),
            Opcode::Tanh => self.intrinsic(args, "tanh"),
            Opcode::Asinh => self.intrinsic(args, "asinh"),
            Opcode::Acosh => self.intrinsic(args, "acosh"),
            Opcode::Atanh => self.intrinsic(args, "atanh"),
            Opcode::Pow => self.intrinsic(args, "llvm.pow.f64"),
            Opcode::OptBarrier => self.values[args[0]].get(self),
            Opcode::Br | Opcode::Jmp | Opcode::Call | Opcode::Phi => unreachable!(),
        };

        let res = self.mir.dfg.first_result(inst);
        self.values[res] = val.into();

        if matches!(
            opcode,
            Opcode::Fneg
                | Opcode::Feq
                | Opcode::Fne
                | Opcode::Fadd
                | Opcode::Fsub
                | Opcode::Fmul
                | Opcode::Fdiv
                | Opcode::Frem
                | Opcode::Flt
                | Opcode::Fgt
                | Opcode::Fle
                | Opcode::Fge
                | Opcode::Sqrt
                | Opcode::Exp
                | Opcode::Ln
                | Opcode::Log
                | Opcode::Clog2
                | Opcode::Floor
                | Opcode::Ceil
                | Opcode::Sin
                | Opcode::Cos
                | Opcode::Tan
                | Opcode::Hypot
                | Opcode::Asin
                | Opcode::Acos
                | Opcode::Atan
                | Opcode::Atan2
                | Opcode::Sinh
                | Opcode::Cosh
                | Opcode::Tanh
                | Opcode::Asinh
                | Opcode::Acosh
                | Opcode::Atanh
                | Opcode::Pow
        ) {
            match fast_math_mode {
                FastMathMode::Full => unsafe { llvm::LLVMSetFastMath(val) },
                FastMathMode::Partial => unsafe { llvm::LLVMSetPartialFastMath(val) },
                FastMathMode::Disabled => (),
            }
        }
    }

    /* Terminator Instructions */

    /// # Safety
    /// * Must not be called multiple times.
    /// * A terminator must not be built for the exit block through other means.
    pub unsafe fn ret(&mut self, val: &'ll llvm::Value) {
        unsafe { llvm::LLVMBuildRet(self.llbuilder, val) };
    }

    /// # Safety
    /// * Must not be called multiple times.
    /// * A terminator must not be built for the exit block through other means.
    pub unsafe fn ret_void(&mut self) {
        unsafe { llvm::LLVMBuildRetVoid(self.llbuilder) };
    }

    /* Memory Access and Addressing Operations */

    /// # Safety
    /// * Must not be called when a block that already contains a terminator is selected.
    /// * Must be called in the entry block of the function
    pub unsafe fn alloca(&self, ty: &'ll llvm::Type) -> &'ll llvm::Value {
        unsafe { llvm::LLVMBuildAlloca(self.llbuilder, ty, UNNAMED) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn store(&self, ptr: &'ll llvm::Value, val: &'ll llvm::Value) {
        unsafe { llvm::LLVMBuildStore(self.llbuilder, val, ptr) };
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn load(&self, ty: &'ll llvm::Type, ptr: &'ll llvm::Value) -> &'ll llvm::Value {
        unsafe { llvm::LLVMBuildLoad2(self.llbuilder, ty, ptr, UNNAMED) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn gep(
        &self,
        elem_ty: &'ll llvm::Type,
        ptr: &'ll llvm::Value,
        indices: &[&'ll llvm::Value],
    ) -> &'ll llvm::Value {
        unsafe {
            llvm::LLVMBuildGEP2(
                self.llbuilder,
                elem_ty,
                ptr,
                indices.as_ptr(),
                indices.len() as u32,
                UNNAMED,
            )
        }
    }

    /// # Safety
    /// * Must not be called when a block that already contains a terminator is selected
    /// * struct_ty must be a valid struct type for this pointer and idx must be in bounds
    pub unsafe fn struct_gep(
        &self,
        struct_ty: &'ll llvm::Type,
        ptr: &'ll llvm::Value,
        idx: u32,
    ) -> &'ll llvm::Value {
        unsafe { llvm::LLVMBuildStructGEP2(self.llbuilder, struct_ty, ptr, idx, UNNAMED) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn fat_ptr_get_ptr(&self, ptr: &'ll llvm::Value) -> &'ll llvm::Value {
        unsafe { self.struct_gep(self.cx.ty_fat_ptr(), ptr, 0) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn fat_ptr_get_meta(&self, ptr: &'ll llvm::Value) -> &'ll llvm::Value {
        unsafe { self.struct_gep(self.cx.ty_fat_ptr(), ptr, 1) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn fat_ptr_to_parts(
        &self,
        ptr: &'ll llvm::Value,
    ) -> (&'ll llvm::Value, &'ll llvm::Value) {
        unsafe { (self.fat_ptr_get_ptr(ptr), self.fat_ptr_get_meta(ptr)) }
    }

    /* Binary Operations */

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn imul(&self, val1: &'ll llvm::Value, val2: &'ll llvm::Value) -> &'ll llvm::Value {
        unsafe { llvm::LLVMBuildMul(self.llbuilder, val1, val2, UNNAMED) }
    }

    /* Other Operations */

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn is_null_ptr(&self, ptr: &'ll llvm::Value) -> &'ll llvm::Value {
        let null_ptr = self.cx.const_null_ptr();
        unsafe {
            llvm::LLVMBuildICmp(self.llbuilder, llvm::IntPredicate::IntEQ, null_ptr, ptr, UNNAMED)
        }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn ptr_diff(
        &self,
        ty: &'ll llvm::Type,
        ptr1: &'ll llvm::Value,
        ptr2: &'ll llvm::Value,
    ) -> &'ll llvm::Value {
        unsafe { llvm::LLVMBuildPtrDiff2(self.llbuilder, ty, ptr1, ptr2, UNNAMED) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    unsafe fn build_int_cmp(
        &mut self,
        args: &[mir::Value],
        predicate: llvm::IntPredicate,
    ) -> &'ll llvm::Value {
        let lhs = self.values[args[0]].get(self);
        let rhs = self.values[args[1]].get(self);
        unsafe { self.int_cmp(lhs, rhs, predicate) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn int_cmp(
        &self,
        lhs: &'ll llvm::Value,
        rhs: &'ll llvm::Value,
        predicate: llvm::IntPredicate,
    ) -> &'ll llvm::Value {
        unsafe { llvm::LLVMBuildICmp(self.llbuilder, predicate, lhs, rhs, UNNAMED) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    unsafe fn build_real_cmp(
        &mut self,
        args: &[mir::Value],
        predicate: llvm::RealPredicate,
    ) -> &'ll llvm::Value {
        let lhs = self.values[args[0]].get(self);
        let rhs = self.values[args[1]].get(self);
        unsafe { self.real_cmp(lhs, rhs, predicate) }
    }

    /// # Safety
    /// Must not be called when a block that already contains a terminator is selected
    pub unsafe fn real_cmp(
        &mut self,
        lhs: &'ll llvm::Value,
        rhs: &'ll llvm::Value,
        predicate: llvm::RealPredicate,
    ) -> &'ll llvm::Value {
        unsafe { llvm::LLVMBuildFCmp(self.llbuilder, predicate, lhs, rhs, UNNAMED) }
    }

    unsafe fn strcmp(&mut self, args: &[mir::Value], invert: bool) -> &'ll llvm::Value {
        let res = self.intrinsic(args, "strcmp");
        let predicate = if invert { llvm::IntPredicate::IntNE } else { llvm::IntPredicate::IntEQ };

        unsafe {
            llvm::LLVMBuildICmp(self.llbuilder, predicate, res, self.cx.const_int(0), UNNAMED)
        }
    }

    /// # Safety
    /// Only correct llvm api calls must be performed within build_then and build_else.
    /// Their return types must match and cond must be a bool.
    pub unsafe fn select(
        &self,
        cond: &'ll llvm::Value,
        then_val: &'ll llvm::Value,
        else_val: &'ll llvm::Value,
    ) -> &'ll llvm::Value {
        unsafe { llvm::LLVMBuildSelect(self.llbuilder, cond, then_val, else_val, UNNAMED) }
    }

    // JW: not used
    // /// # Safety
    // /// * Only correct llvm api calls must be performed within build_then and build_else
    // /// * Their return types must match and cond must be a bool
    // pub unsafe fn add_branching_select(
    //     &mut self,
    //     cond: &'ll llvm::Value,
    //     build_then: impl FnOnce(&mut Self) -> &'ll llvm::Value,
    //     build_else: impl FnOnce(&mut Self) -> &'ll llvm::Value,
    // ) -> &'ll llvm::Value {
    //     let start = self.prepend_pos;
    //     let exit = llvm::LLVMAppendBasicBlockInContext(self.cx.llcx, self.llfunc, UNNAMED);
    //     let then_bb = llvm::LLVMAppendBasicBlockInContext(self.cx.llcx, self.llfunc, UNNAMED);
    //     llvm::LLVMPositionBuilderAtEnd(self.llbuilder, then_bb);
    //     self.prepend_pos = then_bb;
    //     let then_val = build_then(self);
    //     llvm::LLVMBuildBr(self.llbuilder, exit);

    //     let else_bb = llvm::LLVMAppendBasicBlockInContext(self.cx.llcx, self.llfunc, UNNAMED);
    //     llvm::LLVMPositionBuilderAtEnd(self.llbuilder, else_bb);
    //     self.prepend_pos = else_bb;
    //     let else_val = build_else(self);
    //     llvm::LLVMBuildBr(self.llbuilder, exit);

    //     llvm::LLVMPositionBuilderAtEnd(self.llbuilder, start);
    //     llvm::LLVMBuildCondBr(self.llbuilder, cond, then_bb, else_bb);

    //     self.prepend_pos = exit;
    //     llvm::LLVMPositionBuilderAtEnd(self.llbuilder, self.prepend_pos);
    //     let phi = llvm::LLVMBuildPhi(self.llbuilder, llvm::LLVMTypeOf(then_val), UNNAMED);
    //     llvm::LLVMAddIncoming(phi, [then_val, else_val].as_ptr(), [then_bb, else_bb].as_ptr(), 2);

    //     phi
    // }

    /// # Safety
    /// * Must not be called when a block that already contains a terminator is selected
    pub unsafe fn call(
        &self,
        fun_ty: &'ll llvm::Type,
        fun: &'ll llvm::Value,
        operands: &[&'ll llvm::Value],
    ) -> &'ll llvm::Value {
        let res = unsafe {
            llvm::LLVMBuildCall2(
                self.llbuilder,
                fun_ty,
                fun,
                operands.as_ptr(),
                operands.len() as u32,
                UNNAMED,
            )
        };

        // forget this, this is a real footgun
        unsafe {
            let cconv = llvm::LLVMGetFunctionCallConv(fun);
            llvm::LLVMSetInstructionCallConv(res, cconv)
        };

        res
    }

    fn intrinsic(&mut self, args: &[mir::Value], name: &'static str) -> &'ll llvm::Value {
        let (fun_ty, fun) =
            self.cx.intrinsic(name).unwrap_or_else(|| panic!("intrinsic {name} not found"));
        let args: ArrayVec<_, 2> = args.iter().map(|arg| self.values[*arg].get(self)).collect();

        unsafe {
            llvm::LLVMBuildCall2(
                self.llbuilder,
                fun_ty,
                fun,
                args.as_ptr(),
                args.len() as u32,
                UNNAMED,
            )
        }
    }
}
