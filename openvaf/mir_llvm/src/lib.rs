//! Convert from MIR into LLVM IR, and then from LLVM IR into machine code.
//! In general it contains code that runs towards the end of the compilation process.
//!
//! See Also:
//! - [`rustc_codegen_llvm`](https://github.com/rust-lang/rust/tree/master/compiler/rustc_codegen_llvm)

use std::ffi::{c_char, c_void, CStr, CString};
use std::mem::MaybeUninit;
use std::path::Path;
use std::ptr;

use lasso::Rodeo;
use llvm::{
    support::LLVMString, LLVMDisposeMessage, LLVMGetDiagInfoDescription, LLVMGetDiagInfoSeverity,
    LLVMGetHostCPUFeatures, LLVMGetHostCPUName, LLVMPassManagerBuilderDispose,
};
use target::spec::Target;

// #[cfg(test)]
// mod tests;

mod builder;
mod callbacks;
mod context;
mod declarations;
mod intrinsics;
mod types;

pub use builder::{Builder, BuilderVal, MemLoc};
pub use callbacks::CallbackFun;
pub use context::CodegenCx;
pub use llvm::OptLevel;

/// `LLVMBackend` is a factory of LLVM modules and codegen contexts.
pub struct LLVMBackend<'t> {
    target: &'t Target,
    target_cpu: String,
    features: String,
}

impl<'t> LLVMBackend<'t> {
    pub fn new(
        codegen_opts: &[String],
        target: &'t Target,
        target_cpu: &str,
        target_features: &[String],
    ) -> LLVMBackend<'t> {
        let cstr_to_string = |ptr: *const c_char| {
            let string = if ptr.is_null() {
                panic!("could not allocate host CPU features, LLVM returned a `null` string");
            } else {
                unsafe {
                    CStr::from_ptr(ptr)
                        .to_str()
                        .unwrap_or_else(|err| {
                            panic!("LLVM returned a non-utf8 features string: {err}");
                        })
                        .to_owned()
                }
            };
            unsafe { LLVMDisposeMessage(ptr as *mut c_char) };
            string
        };

        let mut features = vec![];

        let target_cpu = match target_cpu {
            "generic" => target.options.cpu.clone(),
            "native" => {
                let ptr = unsafe { LLVMGetHostCPUFeatures() };
                let cpu_features = cstr_to_string(ptr);
                features.extend(cpu_features.split(',').map(String::from));
                let ptr = unsafe { LLVMGetHostCPUName() };
                cstr_to_string(ptr)
            }
            cpu => cpu.to_string(),
        };

        features
            .extend(target.options.features.split(',').filter(|v| !v.is_empty()).map(String::from));
        features.extend(target_features.iter().cloned());

        // TODO add target options here if we ever have any
        let target_opts = &[];
        llvm::init(codegen_opts, target_opts);

        LLVMBackend { target, target_cpu, features: features.join(",") }
    }

    /// # Safety
    ///
    /// This function calls the LLVM-C Api which may not be entirely safe.
    /// Exercise caution!
    pub unsafe fn new_llvm_module(
        &self,
        name: &str,
        opt_lvl: OptLevel,
    ) -> Result<ModuleLlvm, LLVMString> {
        ModuleLlvm::new(name, self.target, &self.target_cpu, &self.features, opt_lvl)
    }

    /// # Safety
    ///
    /// This function calls the LLVM-C Api which may not be entirely safe.
    /// Exercise caution!
    pub unsafe fn new_codegen_context<'a, 'll>(
        &'a self,
        literals: &'a Rodeo,
        llmod: &'ll ModuleLlvm,
    ) -> CodegenCx<'a, 'll> {
        CodegenCx::new(literals, llmod, self.target)
    }

    pub fn target(&self) -> &'t Target {
        self.target
    }
}

// The extern "C" attribute means that some extern C library will call this Rust function
#[no_mangle]
extern "C" fn diagnostic_handler(info: &llvm::DiagnosticInfo, _: *mut c_void) {
    let severity = unsafe { LLVMGetDiagInfoSeverity(info) };
    let msg = unsafe { LLVMString::new(LLVMGetDiagInfoDescription(info)) };
    match severity {
        llvm::DiagnosticSeverity::Error => log::error!("{msg}"),
        llvm::DiagnosticSeverity::Warning => log::warn!("{msg}"),
        llvm::DiagnosticSeverity::Remark => log::debug!("{msg}"),
        llvm::DiagnosticSeverity::Note => log::trace!("{msg}"),
    }
}

/// An LLVM module associated with its LLVM context.
///
/// LLVM programs are composed of `module`s, each of which is a translation unit of the input program.
/// Each module consists of functions, global variables, and symbol table entries. Modules may be
/// combined together with the LLVM linker, which merges function (and global variable) definitions,
/// resolves forward declarations, and merges symbol table entries.
pub struct ModuleLlvm {
    llcx: &'static mut llvm::Context,
    llmod_raw: *const llvm::Module, // must be a raw pointer because the reference must not outlive self/the context
    tm: &'static mut llvm::TargetMachine,
    opt_lvl: OptLevel,
}

impl ModuleLlvm {
    fn new(
        name: &str,
        target: &Target,
        target_cpu: &str,
        features: &str,
        opt_lvl: OptLevel,
    ) -> Result<ModuleLlvm, LLVMString> {
        let llcx = unsafe { llvm::LLVMContextCreate() };
        unsafe {
            llvm::LLVMContextSetDiagnosticHandler(llcx, Some(diagnostic_handler), ptr::null_mut())
        };

        let name = CString::new(name).unwrap();
        let llmod = unsafe { llvm::LLVMModuleCreateWithNameInContext(name.as_ptr(), llcx) };

        let data_layout = CString::new(target.data_layout.as_str()).unwrap();
        unsafe { llvm::LLVMSetDataLayout(llmod, data_layout.as_ptr()) };

        let target_triple = &target.llvm_target;
        let tm = llvm::create_target_machine(
            llmod,
            target_triple,
            target_cpu,
            features,
            opt_lvl,
            llvm::RelocMode::PIC,
            llvm::CodeModel::Default,
        )?;
        let llmod_raw = llmod as _;

        Ok(ModuleLlvm { llcx, llmod_raw, tm, opt_lvl })
    }

    /// Turn the raw pointer to `llvm::Module` into a reference
    pub fn llmod(&self) -> &llvm::Module {
        unsafe { &*self.llmod_raw }
    }

    /// Print the LLVM module to string
    pub fn print(&self) -> LLVMString {
        unsafe { LLVMString::new(llvm::LLVMPrintModuleToString(self.llmod())) }
    }

    /// Set up LLVM pass manager and run optimizations
    pub fn optimize(&self) {
        let llmod = self.llmod();

        unsafe {
            let pmb = llvm::LLVMPassManagerBuilderCreate();
            llvm::pass_manager_builder_set_opt_lvl(pmb, self.opt_lvl);
            llvm::LLVMPassManagerBuilderSetSizeLevel(pmb, 0);

            let fpm = llvm::LLVMCreateFunctionPassManagerForModule(llmod);
            llvm::LLVMPassManagerBuilderPopulateFunctionPassManager(pmb, fpm);
            llvm::run_function_pass_manager(fpm, llmod);
            llvm::LLVMDisposePassManager(fpm);

            let mpm = llvm::LLVMCreatePassManager();
            llvm::LLVMPassManagerBuilderPopulateModulePassManager(pmb, mpm);
            llvm::LLVMRunPassManager(mpm, llmod);
            llvm::LLVMDisposePassManager(mpm);

            LLVMPassManagerBuilderDispose(pmb);
        }
    }

    /// Verifies this LLVM module and prints out any errors to `stderr`.
    ///
    /// # Returns
    /// Whether this module is valid (true if valid)
    pub fn verify_and_print(&self) -> bool {
        unsafe {
            llvm::LLVMVerifyModule(self.llmod(), llvm::VerifierFailureAction::PrintMessage, None)
                == llvm::False
        }
    }

    /// Verifies this LLVM module and retrieve error messages.
    ///
    /// # Returns
    /// An error message in case the module is invalid.
    pub fn verify(&self) -> Option<LLVMString> {
        unsafe {
            let mut err_msg = MaybeUninit::uninit();
            if llvm::LLVMVerifyModule(
                self.llmod(),
                llvm::VerifierFailureAction::ReturnStatus,
                Some(&mut err_msg),
            ) == llvm::True
            {
                Some(err_msg.assume_init())
            } else {
                None
            }
        }
    }

    /// Emits an object file for the given module to `dst` file path.
    pub fn emit_object(&self, dst: &Path) -> Result<(), LLVMString> {
        let path = CString::new(dst.to_str().unwrap()).unwrap();
        let mut err_msg = MaybeUninit::uninit();
        unsafe {
            if llvm::LLVMTargetMachineEmitToFile(
                self.tm,
                self.llmod(),
                path.as_ptr(),
                llvm::CodeGenFileType::ObjectFile,
                err_msg.as_mut_ptr(),
            ) == llvm::True
            {
                Err(LLVMString::new(err_msg.assume_init()))
            } else {
                Ok(())
            }
        }
    }
}

impl Drop for ModuleLlvm {
    fn drop(&mut self) {
        unsafe {
            llvm::LLVMDisposeTargetMachine(&mut *(self.tm as *mut _));
            llvm::LLVMContextDispose(&mut *(self.llcx as *mut _));
        }
    }
}
