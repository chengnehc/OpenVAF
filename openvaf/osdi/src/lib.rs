//! OSDI (Open Source Device Interface)

use std::ffi::CString;

use camino::{Utf8Path, Utf8PathBuf};
use lasso::Rodeo;
use typed_indexmap::TiSet;

use hir::{CompilationDB, ModuleInfo, ParallelDatabase, Type};
use hir_lower::CallBackKind;
use llvm::{Linkage, OptLevel, UnnamedAddr};
use mir_llvm::{CodegenCx, LLVMBackend, ModuleLlvm};
use sim_back::CompiledModule;

mod bitfield;
mod compilation_unit;
mod inst_data;
mod metadata;
mod model_data;

mod access;
mod callbacks;
mod eval;
mod load;
mod setup;

use compilation_unit::{OsdiCompilationUnit, OsdiModule};
use metadata::{
    osdi_0_3::{stdlib_bitcode, OsdiTys},
    OsdiLimFunction,
};

const OSDI_VERSION: (u32, u32) = (0, 3);
const OBJ_NUM: usize = 4;

/// Compile the modules and return intermediate object file paths, which
/// will be passed to linker later on.
pub fn compile<const EMIT: bool>(
    db: &CompilationDB,
    modules: &[ModuleInfo],
    dst: &Utf8Path,
    back: &LLVMBackend,
    opt_lvl: OptLevel,
) -> Vec<Utf8PathBuf> {
    /* Prepare for compilation */
    let name = dst.file_stem().expect("destination should be a file");
    let main_file = dst.with_extension("o");
    let mut paths: Vec<Utf8PathBuf> = (0..modules.len() * OBJ_NUM)
        .map(|i| {
            let num = base_n::encode((i + 1) as u128, base_n::CASE_INSENSITIVE);
            dst.with_extension(format!("o{num}"))
        })
        .collect();

    let target_data = unsafe {
        let src = CString::new(back.target().data_layout.as_str()).unwrap();
        llvm::LLVMCreateTargetData(src.as_ptr())
    };

    /* From HIR module infos to MIR compiled modules */
    let mut literals = Rodeo::new();
    let mut lim_table = TiSet::default();
    let modules: Vec<_> = modules
        .iter()
        .map(|module| {
            let mir = CompiledModule::new(db, module, &mut literals);
            for cb in mir.intern.callbacks.iter() {
                if let CallBackKind::BuiltinLimit { name, num_args } = *cb {
                    lim_table.ensure(OsdiLimFunction { name, num_args: num_args - 2 });
                }
            }
            mir
        })
        .collect();

    /* From MIR to OSDI modules */
    let modules: Vec<_> = modules
        .iter()
        .map(|module| {
            let module = OsdiModule::new(db, module, &lim_table);
            module.intern_names(&mut literals, db);
            module
        })
        .collect();

    let db = db.snapshot();
    rayon_core::scope(|scope| {
        let db = db;
        let literals_ = &literals;
        let target_data_ = &target_data;
        let paths = &paths;

        for (i, module) in modules.iter().enumerate() {
            let db_ = db.snapshot();
            scope.spawn(move |_| {
                let name = format!("access_{}", &module.sym);
                let llmod = unsafe { back.new_llvm_module(&name, opt_lvl).unwrap() };
                let cx = new_codegen(back, &llmod, literals_);
                let tys = OsdiTys::new(&cx, target_data_);
                let cu = OsdiCompilationUnit::new(&db_, module, &cx, &tys, false);

                cu.build_access_fn();
                debug_assert!(llmod.verify_and_print());

                if EMIT {
                    let path = &paths[i * 4];
                    llmod.optimize();
                    assert_eq!(llmod.emit_object(path.as_ref()), Ok(()))
                }
            });

            let db_ = db.snapshot();
            scope.spawn(move |_| {
                let name = format!("setup_model_{}", &module.sym);
                let llmod = unsafe { back.new_llvm_module(&name, opt_lvl).unwrap() };
                let cx = new_codegen(back, &llmod, literals_);
                let tys = OsdiTys::new(&cx, target_data_);
                let cu = OsdiCompilationUnit::new(&db_, module, &cx, &tys, false);

                cu.build_setup_model_fn();
                // std::fs::write(dst.with_extension("setup_model.ll"), llmod.print().to_string()).ok();
                debug_assert!(llmod.verify_and_print());

                if EMIT {
                    let path = &paths[i * 4 + 1];
                    // llmod.optimize();
                    assert_eq!(llmod.emit_object(path.as_ref()), Ok(()))
                }
            });

            let db_ = db.snapshot();
            scope.spawn(move |_| {
                let name = format!("setup_instance_{}", &module.sym);
                let llmod = unsafe { back.new_llvm_module(&name, opt_lvl).unwrap() };
                let cx = new_codegen(back, &llmod, literals_);
                let tys = OsdiTys::new(&cx, target_data_);
                let mut cu = OsdiCompilationUnit::new(&db_, module, &cx, &tys, false);

                cu.build_setup_instance_fn();
                // std::fs::write(dst.with_extension("setup_inst.ll"), llmod.print().to_string()).ok();
                debug_assert!(llmod.verify_and_print());

                if EMIT {
                    let path = &paths[i * 4 + 2];
                    llmod.optimize();
                    assert_eq!(llmod.emit_object(path.as_ref()), Ok(()))
                }
            });

            let db_ = db.snapshot();
            scope.spawn(move |_| {
                let name = format!("eval_{}", &module.sym);
                let llmod = unsafe { back.new_llvm_module(&name, opt_lvl).unwrap() };
                let cx = new_codegen(back, &llmod, literals_);
                let tys = OsdiTys::new(&cx, target_data_);
                let cu = OsdiCompilationUnit::new(&db_, module, &cx, &tys, true);

                // std::fs::write(dst.with_extension("eval.mir"), module.eval.to_debug_string()).ok();
                // println!("{:?}", module.eval);
                cu.build_eval_fn();
                // std::fs::write(dst.with_extension("eval.ll"), llmod.print().to_string()).ok();
                // println!("{}", llmod.to_str());
                debug_assert!(llmod.verify_and_print());

                if EMIT {
                    let path = &paths[i * 4 + 3];
                    llmod.optimize();
                    assert_eq!(llmod.emit_object(path.as_ref()), Ok(()))
                }
            });
        }

        /* Collect and export model metadata */
        let llmod = unsafe { back.new_llvm_module(name, opt_lvl).unwrap() };
        let cx = new_codegen(back, &llmod, &literals);
        let tys = OsdiTys::new(&cx, target_data);
        let descriptors: Vec<_> = modules
            .iter()
            .map(|module| {
                let cu = OsdiCompilationUnit::new(&db, module, &cx, &tys, false);
                let descriptor = cu.descriptor(target_data, &db);
                descriptor.to_ll_val(&cx, &tys)
            })
            .collect();

        cx.export_array("OSDI_DESCRIPTORS", tys.osdi_descriptor, &descriptors, true, false);
        cx.export_val(
            "OSDI_NUM_DESCRIPTORS",
            cx.ty_int(),
            cx.const_unsigned_int(descriptors.len() as u32),
            true,
        );
        cx.export_val(
            "OSDI_VERSION_MAJOR",
            cx.ty_int(),
            cx.const_unsigned_int(OSDI_VERSION.0),
            true,
        );
        cx.export_val(
            "OSDI_VERSION_MINOR",
            cx.ty_int(),
            cx.const_unsigned_int(OSDI_VERSION.1),
            true,
        );

        if !lim_table.is_empty() {
            let lim: Vec<_> = lim_table.iter().map(|entry| entry.to_ll_val(&cx, &tys)).collect();
            cx.export_array("OSDI_LIM_TABLE", tys.osdi_lim_function, &lim, false, false);
            cx.export_val(
                "OSDI_LIM_TABLE_LEN",
                cx.ty_int(),
                cx.const_unsigned_int(lim.len() as u32),
                true,
            );
        }

        let osdi_log =
            cx.get_global_value("osdi_log").expect("symbol osdi_log missing from std lib");
        unsafe {
            llvm::LLVMSetInitializer(osdi_log, cx.const_null_ptr());
            llvm::LLVMSetLinkage(osdi_log, llvm::Linkage::ExternalLinkage);
            llvm::LLVMSetUnnamedAddress(osdi_log, llvm::UnnamedAddr::No);
            llvm::LLVMSetDLLStorageClass(osdi_log, llvm::DLLStorageClass::Export);
        }

        // std::fs::write(dst.with_extension("ll"), llmod.print().to_string()).ok();
        debug_assert!(llmod.verify_and_print());

        if EMIT {
            // println!("{}", llmod.to_str());
            llmod.optimize();
            // println!("{}", llmod.to_str());
            assert_eq!(llmod.emit_object(main_file.as_ref()), Ok(()))
        }
    });

    unsafe { llvm::LLVMDisposeTargetData(target_data) };
    paths.push(main_file);
    paths
}

fn new_codegen<'a, 'll>(
    backend: &'a LLVMBackend,
    llmod: &'ll ModuleLlvm,
    literals: &'a Rodeo,
) -> CodegenCx<'a, 'll> {
    let cx = unsafe { backend.new_codegen_context(literals, llmod) };
    cx.include_bitcode(stdlib_bitcode(backend.target()));

    // Allow functions in stdlib to be merged
    for fun in llvm::function_iter(llmod.llmod()) {
        unsafe {
            // LLVMPurgeAttrs(fun);
            if llvm::LLVMIsDeclaration(fun) != llvm::False {
                continue; // skip function declarations
            }
            llvm::LLVMSetLinkage(fun, Linkage::Internal);
            llvm::LLVMSetUnnamedAddress(fun, UnnamedAddr::Global);
        }
    }
    let scale_factor_table =
        cx.get_global_value("SCALE_FACTORS").expect("constant SCALE_FACTORS missing from stdlib");
    let scale_symbol_table =
        cx.get_global_value("SCALE_SYMBOLS").expect("constant SCALE_SYMBOLS missing from stdlib");

    // Allow these variables to be merged
    unsafe {
        llvm::LLVMSetLinkage(scale_factor_table, Linkage::Internal);
        llvm::LLVMSetLinkage(scale_symbol_table, Linkage::Internal);
    }
    cx
}

/// Get the corresponding LLVM type of an HIR type
fn lltype<'ll>(ty: &Type, cx: &CodegenCx<'_, 'll>) -> &'ll llvm::Type {
    let llty = match ty.base_type() {
        Type::Void => cx.ty_void(),
        Type::Bool => cx.ty_c_bool(),
        Type::Integer => cx.ty_int(),
        Type::Real => cx.ty_double(),
        Type::String => cx.ty_ptr(),
        Type::EmptyArray => cx.ty_array(cx.ty_int(), 0),
        Type::Err | Type::Array { .. } => unreachable!(),
    };

    if let Some(len) = ty_len(ty) {
        cx.ty_array(llty, len)
    } else {
        llty
    }
}

/// Get the length of an HIR array type
fn ty_len(ty: &Type) -> Option<u32> {
    match ty {
        Type::Array { ty, len } => Some(len * ty_len(ty).unwrap_or(1)),
        Type::EmptyArray => Some(0),
        _ => None,
    }
}
