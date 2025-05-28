use hir::{CompilationDB, ModuleInfo, ParamSysFun};
use hir_lower::fmt::{DisplayKind, FmtArg, FmtArgKind};
use hir_lower::{CallBackKind, HirInterner, ParamKind};
use lasso::Rodeo;
use llvm::{
    IntPredicate, LLVMAddIncoming, LLVMAppendBasicBlockInContext, LLVMBuildAdd,
    LLVMBuildArrayMalloc, LLVMBuildBr, LLVMBuildCall2, LLVMBuildCondBr, LLVMBuildFMul,
    LLVMBuildFree, LLVMBuildICmp, LLVMBuildInBoundsGEP2, LLVMBuildLoad2, LLVMBuildPhi,
    LLVMGetParam, LLVMIsDeclaration, LLVMPositionBuilderAtEnd, LLVMSetDLLStorageClass,
    LLVMSetLinkage, LLVMSetUnnamedAddress, Linkage, UnnamedAddr, UNNAMED,
};
use mir::{FuncRef, Function};
use mir_llvm::{CallbackFun, CodegenCx, LLVMBackend, ModuleLlvm};
use sim_back::dae::DaeSystem;
use sim_back::init::Initialization;
use sim_back::node_collapse::NodeCollapse;
use sim_back::CompiledModule;
use typed_index_collections::TiVec;
use typed_indexmap::TiSet;

use crate::inst_data::OsdiInstanceData;
use crate::metadata::osdi_0_3::{
    stdlib_bitcode, OsdiTys, LOG_FMT_ERR, LOG_LVL_DEBUG, LOG_LVL_DISPLAY, LOG_LVL_ERR,
    LOG_LVL_FATAL, LOG_LVL_INFO, LOG_LVL_WARN,
};
use crate::metadata::{sim_unknown_info, OsdiLimFunction};
use crate::model_data::OsdiModelData;
use crate::{lltype, OsdiLimId};

pub fn new_codegen<'a, 'll>(
    backend: &'a LLVMBackend,
    llmod: &'ll ModuleLlvm,
    literals: &'a Rodeo,
) -> CodegenCx<'a, 'll> {
    let cx = unsafe { backend.new_codegen_context(literals, llmod) };
    cx.include_bitcode(stdlib_bitcode(backend.target()));

    for fun in llvm::function_iter(llmod.llmod()) {
        unsafe {
            // LLVMPurgeAttrs(fun);
            if LLVMIsDeclaration(fun) != llvm::False {
                continue; // skip function declarations
            }
            LLVMSetLinkage(fun, Linkage::Internal);
            LLVMSetUnnamedAddress(fun, UnnamedAddr::Global);
        }
    }
    let exp_table = cx.get_declared_value("EXP").expect("constant EXP missing from stdlib");
    let char_table =
        cx.get_declared_value("FMT_CHARS").expect("constant FMT_CHARS missing from stdlib");
    unsafe {
        LLVMSetLinkage(exp_table, Linkage::Internal);
        LLVMSetLinkage(char_table, Linkage::Internal);
    }
    cx
}

pub struct OsdiCompilationUnit<'a, 'b, 'll> {
    pub db: &'a CompilationDB,
    pub cx: &'a CodegenCx<'b, 'll>,
    pub tys: &'a OsdiTys<'ll>,
    pub module: &'a OsdiModule<'b>,
    pub inst_data: OsdiInstanceData<'ll>,
    pub model_data: OsdiModelData<'ll>,
    pub lim_dispatch_table: Option<&'ll llvm::Value>,
}

impl<'a, 'b, 'll> OsdiCompilationUnit<'a, 'b, 'll> {
    pub fn new(
        db: &'a CompilationDB,
        module: &'a OsdiModule<'b>,
        cx: &'a CodegenCx<'b, 'll>,
        tys: &'a OsdiTys<'ll>,
        is_eval: bool,
    ) -> OsdiCompilationUnit<'a, 'b, 'll> {
        let inst_data = OsdiInstanceData::new(db, module, cx);
        let model_data = OsdiModelData::new(db, module, cx, &inst_data);
        let lim_dispatch_table =
            if is_eval && !module.lim_table.is_empty() && !module.intern.lim_state.is_empty() {
                let ty = cx.ty_array(tys.osdi_lim_function, module.lim_table.len() as u32);
                let ptr = cx
                    .define_global("OSDI_LIM_TABLE", ty)
                    .unwrap_or_else(|| unreachable!("symbol OSDI_LIM_TABLE already defined"));
                unsafe {
                    LLVMSetLinkage(ptr, llvm::Linkage::ExternalLinkage);
                    LLVMSetUnnamedAddress(ptr, llvm::UnnamedAddr::No);
                    LLVMSetDLLStorageClass(ptr, llvm::DLLStorageClass::Export);
                }
                Some(ptr)
            } else {
                None
            };
        OsdiCompilationUnit { db, tys, cx, module, inst_data, model_data, lim_dispatch_table }
    }
}

pub struct OsdiModule<'a> {
    pub info: &'a ModuleInfo,
    pub dae: &'a DaeSystem,
    pub init: &'a Initialization, // setup_instance
    pub node_collapse: &'a NodeCollapse,
    pub eval: &'a Function,
    pub intern: &'a HirInterner,
    pub model_param_setup: &'a Function, // setup_model
    pub model_param_intern: &'a HirInterner,
    pub lim_table: &'a TiSet<OsdiLimId, OsdiLimFunction>,
    pub sym: String,
}

impl<'a> OsdiModule<'a> {
    pub fn new(
        db: &'a CompilationDB,
        module: &'a CompiledModule,
        lim_table: &'a TiSet<OsdiLimId, OsdiLimFunction>,
    ) -> Self {
        let sym = base_n::encode(module.info.module.uuid(db) as u128, base_n::CASE_INSENSITIVE);
        let CompiledModule {
            info,
            dae,
            eval,
            intern,
            init,
            model_param_setup,
            model_param_intern,
            node_collapse,
        } = module;
        OsdiModule {
            sym,
            lim_table,
            info,
            dae,
            eval,
            intern,
            init,
            model_param_setup,
            model_param_intern,
            node_collapse,
        }
    }

    pub fn intern_names(&self, literals: &mut Rodeo, db: &CompilationDB) {
        literals.get_or_intern(self.info.module.name(db));
        self.intern_unknown_names(literals, db);
        literals.get_or_intern_static("Multiplier (Verilog-A $mfactor)");
        literals.get_or_intern_static("deg");
        literals.get_or_intern_static("m");
        literals.get_or_intern_static("");

        for param in self.info.params.values() {
            for alias in &param.aliases {
                literals.get_or_intern(&**alias);
            }
            literals.get_or_intern(&param.name);
            literals.get_or_intern(&param.units);
            literals.get_or_intern(&param.desc);
            literals.get_or_intern(&param.group);
        }

        for aliases in self.info.param_sysfuns.values() {
            for alias in aliases {
                literals.get_or_intern(&**alias);
            }
        }

        for param in ParamSysFun::iter() {
            let is_live = |intern: &HirInterner, func| {
                intern.is_param_live(func, &ParamKind::ParamSysFun(param))
            };
            if is_live(self.intern, self.eval)
                || is_live(&self.init.intern, &self.init.func)
                || is_live(self.model_param_intern, self.model_param_setup)
            {
                literals.get_or_intern(format!("${param:?}"));
            }
        }

        for (var, opvar_info) in self.info.op_vars.iter() {
            literals.get_or_intern(var.name(db));
            literals.get_or_intern(&opvar_info.units);
            literals.get_or_intern(&opvar_info.desc);
        }
    }

    pub fn intern_unknown_names(&self, interner: &mut Rodeo, db: &CompilationDB) {
        for &unknown in self.dae.unknowns.iter() {
            let (name, units, _) = sim_unknown_info(unknown, db);
            interner.get_or_intern(&name);
            interner.get_or_intern(&units);
        }
    }
}

/// Common callbacks shared among functions
pub fn general_callbacks<'ll>(
    intern: &HirInterner,
    builder: &mir_llvm::Builder<'_, '_, 'll>,
    ret_flags: &'ll llvm::Value,
    handle: &'ll llvm::Value,
    simparam: &'ll llvm::Value,
) -> TiVec<FuncRef, Option<CallbackFun<'ll>>> {
    let ptr_ty = builder.cx.ty_ptr();
    let double_ty = builder.cx.ty_double();
    intern
        .callbacks
        .raw
        .iter()
        .map(|call| {
            let cb = match call {
                CallBackKind::SimParam => {
                    let fun = builder
                        .cx
                        .get_func_by_name("simparam")
                        .expect("stdlib function simparam is missing");
                    let fun_ty = builder.cx.ty_func(&[ptr_ty; 4], double_ty);
                    CallbackFun {
                        fun_ty,
                        fun,
                        state: Box::new([simparam, handle, ret_flags]),
                        num_state: 0,
                    }
                }
                CallBackKind::SimParamOpt => {
                    let fun = builder
                        .cx
                        .get_func_by_name("simparam_opt")
                        .expect("stdlib function simparam_opt is missing");
                    let fun_ty = builder.cx.ty_func(&[ptr_ty, ptr_ty, double_ty], double_ty);
                    CallbackFun { fun_ty, fun, state: Box::new([simparam]), num_state: 0 }
                }
                CallBackKind::SimParamStr => {
                    let fun = builder
                        .cx
                        .get_func_by_name("simparam_str")
                        .expect("stdlib function simparam_str is missing");
                    let fun_ty = builder.cx.ty_func(&[ptr_ty; 4], ptr_ty);
                    CallbackFun {
                        fun_ty,
                        fun,
                        state: Box::new([simparam, handle, ret_flags]),
                        num_state: 0,
                    }
                }
                CallBackKind::Print { kind, arg_tys } => {
                    let (fun, fun_ty) = print_callback(builder.cx, *kind, arg_tys);
                    CallbackFun { fun_ty, fun, state: Box::new([handle]), num_state: 0 }
                }
                // If these derivative were non-zero, they would have been removed
                CallBackKind::Derivative(_) | CallBackKind::NodeDerivative(_) => {
                    let zero = builder.cx.const_real(0.0);
                    builder.cx.const_callback(&[double_ty], zero)
                }
                CallBackKind::ParamInfo(_, _)
                | CallBackKind::CollapseHint(_, _)
                | CallBackKind::BuiltinLimit { .. }
                | CallBackKind::StoreLimit(_)
                | CallBackKind::LimDiscontinuity
                | CallBackKind::Analysis
                | CallBackKind::NoiseTable(_)
                | CallBackKind::WhiteNoise { .. }
                | CallBackKind::FlickerNoise { .. }
                | CallBackKind::TimeDerivative => return None,
            };
            Some(cb)
        })
        .collect()
}

fn print_callback<'ll>(
    cx: &CodegenCx<'_, 'll>,
    kind: hir_lower::fmt::DisplayKind,
    arg_tys: &[FmtArg],
) -> (&'ll llvm::Value, &'ll llvm::Type) {
    let mut args = vec![cx.ty_ptr(), cx.ty_ptr()];
    args.extend(arg_tys.iter().map(|arg| lltype(&arg.ty, cx)));
    let fun_ty = cx.ty_func(&args, cx.ty_void());
    let name = cx.local_callback_name();
    let fun = cx.declare_internal_fn(&name, fun_ty);
    unsafe {
        let entry_bb = LLVMAppendBasicBlockInContext(cx.llcx, fun, UNNAMED);
        let alloc_bb = LLVMAppendBasicBlockInContext(cx.llcx, fun, UNNAMED);
        let write_bb = LLVMAppendBasicBlockInContext(cx.llcx, fun, UNNAMED);
        let err_bb = LLVMAppendBasicBlockInContext(cx.llcx, fun, UNNAMED);
        let exit_bb = LLVMAppendBasicBlockInContext(cx.llcx, fun, UNNAMED);
        let llbuilder = llvm::LLVMCreateBuilderInContext(cx.llcx);

        LLVMPositionBuilderAtEnd(llbuilder, entry_bb);
        let handle = LLVMGetParam(fun, 0);
        let fmt_lit = LLVMGetParam(fun, 1);
        let mut args = vec![cx.const_null_ptr(), cx.const_usize(0), LLVMGetParam(fun, 1)];

        let exp_table = cx.get_declared_value("EXP").expect("constant EXP missing from stdlib");
        let exp_table_ty = cx.ty_array(cx.ty_double(), 11);
        let char_table =
            cx.get_declared_value("FMT_CHARS").expect("constant FMT_CHARS missing from stdlib");
        let char_table_ty = cx.ty_array(cx.ty_char(), 11);
        let fmt_char_idx =
            cx.get_func_by_name("fmt_char_idx").expect("fmt_char_idx missing from stdlib");
        let fmt_char_idx_ty = cx.ty_func(&[cx.ty_double()], cx.ty_int());
        let fmt_binary = cx.get_func_by_name("fmt_binary").expect("fmt_binary missing from stdlib");
        let fmt_binary_ty = cx.ty_func(&[cx.ty_int()], cx.ty_ptr());
        let mut free = Vec::new();

        for (i, arg) in arg_tys.iter().enumerate() {
            let val = LLVMGetParam(fun, i as u32 + 2);
            match arg.kind {
                FmtArgKind::Binary => {
                    let formatted_str = LLVMBuildCall2(
                        llbuilder,
                        fmt_binary_ty,
                        fmt_binary,
                        [val].as_ptr(),
                        1,
                        UNNAMED,
                    );
                    free.push(formatted_str);
                }
                FmtArgKind::EngineerReal => {
                    let idx = LLVMBuildCall2(
                        llbuilder,
                        fmt_char_idx_ty,
                        fmt_char_idx,
                        [val].as_ptr(),
                        1,
                        UNNAMED,
                    );
                    let exp = LLVMBuildInBoundsGEP2(
                        llbuilder,
                        exp_table_ty,
                        exp_table,
                        [cx.const_int(0), idx].as_ptr(),
                        2,
                        UNNAMED,
                    );
                    let exp = LLVMBuildLoad2(llbuilder, cx.ty_double(), exp, UNNAMED);
                    let num = LLVMBuildFMul(llbuilder, val, exp, UNNAMED);
                    args.push(num);
                    let scale_char = LLVMBuildInBoundsGEP2(
                        llbuilder,
                        char_table_ty,
                        char_table,
                        [cx.const_int(0), idx].as_ptr(),
                        2,
                        UNNAMED,
                    );
                    args.push(scale_char);
                }
                FmtArgKind::Other => args.push(val),
            }
        }
        args.extend((1..(2 + arg_tys.len())).map(|arg| LLVMGetParam(fun, arg as u32)));
        let (fun_ty, fun) = cx.intrinsic("snprintf").unwrap();
        let len = LLVMBuildCall2(llbuilder, fun_ty, fun, args.as_ptr(), args.len() as u32, UNNAMED);
        let is_err = LLVMBuildICmp(llbuilder, IntPredicate::IntSLT, len, cx.const_int(0), UNNAMED);
        LLVMBuildCondBr(llbuilder, is_err, err_bb, alloc_bb);

        LLVMPositionBuilderAtEnd(llbuilder, alloc_bb);
        let data_len = LLVMBuildAdd(llbuilder, len, cx.const_int(1), UNNAMED);
        let ptr = LLVMBuildArrayMalloc(llbuilder, cx.ty_char(), data_len, UNNAMED);
        let null_ptr = cx.const_null_ptr();
        let is_err = LLVMBuildICmp(llbuilder, llvm::IntPredicate::IntEQ, null_ptr, ptr, UNNAMED);
        LLVMBuildCondBr(llbuilder, is_err, err_bb, write_bb);

        LLVMPositionBuilderAtEnd(llbuilder, write_bb);
        let data_len = LLVMBuildAdd(llbuilder, len, cx.const_int(1), UNNAMED);
        args[0] = ptr;
        args[1] = data_len;
        let len = LLVMBuildCall2(llbuilder, fun_ty, fun, args.as_ptr(), args.len() as u32, UNNAMED);
        let is_err = LLVMBuildICmp(llbuilder, IntPredicate::IntSLT, len, cx.const_int(0), UNNAMED);
        for alloc in free.iter() {
            LLVMBuildFree(llbuilder, alloc);
        }
        LLVMBuildCondBr(llbuilder, is_err, err_bb, exit_bb);

        LLVMPositionBuilderAtEnd(llbuilder, err_bb);
        LLVMBuildBr(llbuilder, exit_bb);

        LLVMPositionBuilderAtEnd(llbuilder, exit_bb);
        let flags = LLVMBuildPhi(llbuilder, cx.ty_int(), UNNAMED);
        let lvl = match kind {
            DisplayKind::Debug => LOG_LVL_DEBUG,
            DisplayKind::Display | DisplayKind::Monitor => LOG_LVL_DISPLAY,
            DisplayKind::Info => LOG_LVL_INFO,
            DisplayKind::Warn => LOG_LVL_WARN,
            DisplayKind::Error => LOG_LVL_ERR,
            DisplayKind::Fatal => LOG_LVL_FATAL,
        };
        let lvl_and_err = lvl | LOG_FMT_ERR;
        let lvl = cx.const_unsigned_int(lvl);
        let lvl_and_err = cx.const_unsigned_int(lvl_and_err);
        LLVMAddIncoming(flags, [lvl, lvl_and_err].as_ptr(), [write_bb, err_bb].as_ptr(), 2);
        let msg = LLVMBuildPhi(llbuilder, cx.ty_ptr(), UNNAMED);
        LLVMAddIncoming(msg, [ptr, fmt_lit].as_ptr(), [write_bb, err_bb].as_ptr(), 2);
        let fun_ptr = cx.get_declared_value("osdi_log").expect("symbol osdi_log is missing");
        let fun_ty = cx.ty_func(&[cx.ty_ptr(), cx.ty_ptr(), cx.ty_int()], cx.ty_void());
        let fun = LLVMBuildLoad2(llbuilder, cx.ty_ptr(), fun_ptr, UNNAMED);
        LLVMBuildCall2(llbuilder, fun_ty, fun, [handle, msg, flags].as_ptr(), 3, UNNAMED);
        llvm::LLVMBuildRetVoid(llbuilder);
        llvm::LLVMDisposeBuilder(llbuilder);
    }

    (fun, fun_ty)
}
