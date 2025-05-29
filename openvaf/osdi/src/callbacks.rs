use typed_index_collections::TiVec;

use hir_lower::{
    fmt::{DisplayKind, FmtArg, FmtArgKind},
    CallBackKind, HirInterner,
};
use llvm::{
    IntPredicate, LLVMAddIncoming, LLVMAppendBasicBlockInContext, LLVMBuildAdd,
    LLVMBuildArrayMalloc, LLVMBuildBr, LLVMBuildCall2, LLVMBuildCondBr, LLVMBuildFMul,
    LLVMBuildFree, LLVMBuildICmp, LLVMBuildInBoundsGEP2, LLVMBuildLoad2, LLVMBuildPhi,
    LLVMGetParam, LLVMPositionBuilderAtEnd, UNNAMED,
};
use mir::FuncRef;
use mir_llvm::{CallbackFun, CodegenCx};

use crate::lltype;
use crate::metadata::osdi_0_3::{
    LOG_FMT_ERR, LOG_LVL_DEBUG, LOG_LVL_DISPLAY, LOG_LVL_ERR, LOG_LVL_FATAL, LOG_LVL_INFO,
    LOG_LVL_WARN,
};

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
                CallBackKind::Print { kind, fmt_args } => {
                    let (fun, fun_ty) = print_callback(builder.cx, *kind, fmt_args);
                    CallbackFun { fun_ty, fun, state: Box::new([handle]), num_state: 0 }
                }
                // If these derivative were non-zero, they would have been removed
                CallBackKind::Derivative(_) | CallBackKind::NodeDerivative(_) => {
                    let zero = builder.cx.const_real(0.0);
                    builder.cx.const_callback(&[double_ty], zero)
                }
                _ => return None,
            };
            Some(cb)
        })
        .collect()
}

/// Format display callbacks
fn print_callback<'ll>(
    cx: &CodegenCx<'_, 'll>,
    kind: DisplayKind,
    fmt_args: &[FmtArg],
) -> (&'ll llvm::Value, &'ll llvm::Type) {
    // args[0]: handle
    // args[1]: fmt string literal
    // args[2..]: fmt args
    let mut args = vec![cx.ty_ptr(), cx.ty_ptr()];
    args.extend(fmt_args.iter().map(|arg| lltype(&arg.ty, cx)));

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

        // args[0]: string buffer ptr
        // args[1]: string data length
        let mut args = vec![cx.const_null_ptr(), cx.const_usize(0), fmt_lit];

        let scale_factor_table = cx
            .get_global_value("SCALE_FACTORS")
            .expect("constant SCALE_FACTORS missing from stdlib");
        let scale_factor_table_ty = cx.ty_array(cx.ty_double(), 11);
        let scale_symbol_table = cx
            .get_global_value("SCALE_SYMBOLS")
            .expect("constant SCALE_SYMBOLS missing from stdlib");
        let scale_symbol_table_ty = cx.ty_array(cx.ty_char(), 11);

        let get_scale_idx = cx
            .get_func_by_name("get_scale_idx")
            .expect("function get_scale_idx missing from stdlib");
        let get_scale_idx_ty = cx.ty_func(&[cx.ty_double()], cx.ty_int());

        let fmt_binary =
            cx.get_func_by_name("fmt_binary").expect("function fmt_binary missing from stdlib");
        let fmt_binary_ty = cx.ty_func(&[cx.ty_int()], cx.ty_ptr());

        let mut free = Vec::new();
        for (i, fmt_arg) in fmt_args.iter().enumerate() {
            let val = LLVMGetParam(fun, i as u32 + 2);
            match fmt_arg.kind {
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
                        get_scale_idx_ty,
                        get_scale_idx,
                        [val].as_ptr(),
                        1,
                        UNNAMED,
                    );
                    let scale_factor = LLVMBuildInBoundsGEP2(
                        llbuilder,
                        scale_factor_table_ty,
                        scale_factor_table,
                        [cx.const_int(0), idx].as_ptr(),
                        2,
                        UNNAMED,
                    );
                    let scale_factor =
                        LLVMBuildLoad2(llbuilder, cx.ty_double(), scale_factor, UNNAMED);
                    // scale the original value with factor
                    let scaled_val = LLVMBuildFMul(llbuilder, val, scale_factor, UNNAMED);
                    args.push(scaled_val);
                    let scale_symbol = LLVMBuildInBoundsGEP2(
                        llbuilder,
                        scale_symbol_table_ty,
                        scale_symbol_table,
                        [cx.const_int(0), idx].as_ptr(),
                        2,
                        UNNAMED,
                    );
                    args.push(scale_symbol);
                }
                FmtArgKind::Other => args.push(val),
            }
        }
        args.extend((1..(2 + fmt_args.len())).map(|arg| LLVMGetParam(fun, arg as u32)));
        let (fun_ty, fun) = cx.intrinsic("snprintf").unwrap();

        let len = LLVMBuildCall2(llbuilder, fun_ty, fun, args.as_ptr(), args.len() as u32, UNNAMED);
        let is_err = LLVMBuildICmp(llbuilder, IntPredicate::IntSLT, len, cx.const_int(0), UNNAMED);
        LLVMBuildCondBr(llbuilder, is_err, err_bb, alloc_bb);

        LLVMPositionBuilderAtEnd(llbuilder, alloc_bb);
        // plus the terminating null character
        let data_len = LLVMBuildAdd(llbuilder, len, cx.const_int(1), UNNAMED);
        let ptr = LLVMBuildArrayMalloc(llbuilder, cx.ty_char(), data_len, UNNAMED);
        // check if `malloc` fails
        let is_err =
            LLVMBuildICmp(llbuilder, IntPredicate::IntEQ, cx.const_null_ptr(), ptr, UNNAMED);
        LLVMBuildCondBr(llbuilder, is_err, err_bb, write_bb);

        LLVMPositionBuilderAtEnd(llbuilder, write_bb);
        let data_len = LLVMBuildAdd(llbuilder, len, cx.const_int(1), UNNAMED);
        args[0] = ptr;
        args[1] = data_len;
        // `snprinf` returns negative number when error occurs
        let len = LLVMBuildCall2(llbuilder, fun_ty, fun, args.as_ptr(), args.len() as u32, UNNAMED);
        let is_err = LLVMBuildICmp(llbuilder, IntPredicate::IntSLT, len, cx.const_int(0), UNNAMED);
        for alloc in free {
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

        let fun_ptr = cx.get_global_value("osdi_log").expect("symbol osdi_log is missing");
        let fun_ty = cx.ty_func(&[cx.ty_ptr(), cx.ty_ptr(), cx.ty_int()], cx.ty_void());
        let fun = LLVMBuildLoad2(llbuilder, cx.ty_ptr(), fun_ptr, UNNAMED);
        LLVMBuildCall2(llbuilder, fun_ty, fun, [handle, msg, flags].as_ptr(), 3, UNNAMED);
        llvm::LLVMBuildRetVoid(llbuilder);
        llvm::LLVMDisposeBuilder(llbuilder);
    }

    (fun, fun_ty)
}
