use std::cell::{Cell, RefCell};
use std::ffi::{c_char, c_uint, CString};

use ahash::AHashMap;
use lasso::{Rodeo, Spur};
use llvm::{
    support::LLVMString, LLVMCreateMemoryBufferWithMemoryRange, LLVMGetNamedFunction,
    LLVMLinkModules2, LLVMParseBitcodeInContext2, Type, Value,
};
use target::spec::Target;

use crate::types::Types;

pub struct CodegenCx<'a, 'll> {
    pub llmod: &'ll llvm::Module,
    pub llcx: &'ll llvm::Context,
    pub target: &'a Target,
    pub literals: &'a Rodeo,
    str_lit_cache: RefCell<AHashMap<Spur, &'ll Value>>,
    pub intrinsics: RefCell<AHashMap<&'static str, (&'ll Type, &'ll Value)>>,
    pub local_gen_sym_counter: Cell<u32>,
    pub tys: Types<'ll>,
}

impl<'a, 'll> CodegenCx<'a, 'll> {
    pub fn new(
        literals: &'a Rodeo,
        module_llvm: &'ll crate::ModuleLlvm,
        target: &'a Target,
    ) -> CodegenCx<'a, 'll> {
        // let ty_isize =
        //     unsafe { llvm::LLVMIntTypeInContext(llvm_module.llcx, target.pointer_width) };
        CodegenCx {
            llmod: module_llvm.llmod(),
            llcx: module_llvm.llcx,
            target,
            literals,
            str_lit_cache: RefCell::new(AHashMap::with_capacity(literals.len())),
            intrinsics: RefCell::new(AHashMap::new()),
            local_gen_sym_counter: Cell::new(0),
            tys: Types::new(module_llvm.llcx, target.pointer_width),
        }
    }

    pub fn get_func_by_name(&self, name: &str) -> Option<&'ll llvm::Value> {
        let name = CString::new(name).unwrap();
        unsafe { LLVMGetNamedFunction(self.llmod, name.as_ptr()) }
    }

    pub fn include_bitcode(&self, bitcode: &[u8]) {
        let sym = self.generate_local_symbol_name("bitcode_buffer");
        let sym = CString::new(sym).unwrap();
        unsafe {
            let buf = LLVMCreateMemoryBufferWithMemoryRange(
                bitcode.as_ptr() as *const c_char,
                bitcode.len(),
                sym.as_ptr(),
                llvm::False,
            );
            let mut module = None;
            assert!(
                LLVMParseBitcodeInContext2(self.llcx, buf, &mut module) == llvm::False,
                "failed to parse bitcode"
            );
            assert!(
                LLVMLinkModules2(self.llmod, module.unwrap()) == llvm::False,
                "failed to link parsed bitcode"
            );
        }
    }

    pub fn to_str(&self) -> LLVMString {
        unsafe { LLVMString::new(llvm::LLVMPrintModuleToString(self.llmod)) }
    }

    pub fn const_str_uninterned(&self, lit: &str) -> &'ll Value {
        let lit = self.literals.get(lit).unwrap();
        self.const_str(lit)
    }

    pub fn const_str(&self, lit: Spur) -> &'ll Value {
        // fast path: check if string literal is cached
        if let Some(val) = self.str_lit_cache.borrow().get(&lit) {
            return val;
        }

        let val = self.literals.resolve(&lit).as_bytes();
        let val = unsafe {
            llvm::LLVMConstStringInContext(
                self.llcx,
                val.as_ptr() as *const c_char,
                val.len() as c_uint,
                false as llvm::Bool,
            )
        };
        let name = self.generate_local_symbol_name("str");
        let ty = self.ty_of(val);
        let global = self
            .define_global(&name, ty)
            .unwrap_or_else(|| unreachable!("symbol {name} already defined"));
        unsafe {
            llvm::LLVMSetInitializer(global, val);
            llvm::LLVMSetGlobalConstant(global, llvm::True);
            llvm::LLVMSetLinkage(global, llvm::Linkage::Internal);
        }
        // cache the string literal global value
        self.str_lit_cache.borrow_mut().insert(lit, global);

        global
    }
}

impl CodegenCx<'_, '_> {
    /// Generates a new symbol name with `prefix`. This symbol name must
    /// only be used for definitions with `internal` or `private` linkage.
    pub fn generate_local_symbol_name(&self, prefix: &str) -> String {
        let idx = self.local_gen_sym_counter.get();
        self.local_gen_sym_counter.set(idx + 1);

        let mut name = String::with_capacity(prefix.len() + 6);
        name.push_str(prefix);
        name.push('.'); // append a '.' to avoid accidental conflict with user defined names
        base_n::push_str(idx as u128, base_n::ALPHANUMERIC_ONLY, &mut name);

        name
    }
}
