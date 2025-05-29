use std::ffi::CString;

use llvm::{Type, Value};

use crate::CodegenCx;

/// Declare a function.
///
/// If there’s a value with the same name already declared,
/// update the declaration and return existing Value instead.
pub fn declare_raw_fn<'ll>(
    cx: &CodegenCx<'_, 'll>,
    name: &str,
    callconv: llvm::CallConv,
    unnamed: llvm::UnnamedAddr,
    func_ty: &'ll Type,
) -> &'ll Value {
    let name = CString::new(name).unwrap();
    unsafe {
        let llfn = llvm::LLVMAddFunction(cx.llmod, name.as_ptr(), func_ty);
        llvm::LLVMSetFunctionCallConv(llfn, callconv);
        llvm::LLVMSetUnnamedAddress(llfn, unnamed);

        llfn
    }
}

impl<'ll> CodegenCx<'_, 'll> {
    /// Declare a C ABI function.
    ///
    /// Only use this for foreign function ABIs and glue. For Rust functions use
    /// `declare_fn` instead.
    ///
    /// If there’s a value with the same name already declared, the function will
    /// update the declaration and return existing Value instead.
    pub fn declare_external_fn(&self, name: &str, fn_type: &'ll Type) -> &'ll Value {
        declare_raw_fn(self, name, llvm::CallConv::CCallConv, llvm::UnnamedAddr::No, fn_type)
    }

    /// Declare a internal function.
    pub fn declare_internal_fn(&self, name: &str, fn_type: &'ll Type) -> &'ll Value {
        let fun = declare_raw_fn(
            self,
            name,
            llvm::CallConv::FastCallConv,
            llvm::UnnamedAddr::Global, // Function addresses are never significant, allow functions to be merged
            fn_type,
        );
        unsafe { llvm::LLVMSetLinkage(fun, llvm::Linkage::Internal) }
        fun
    }

    /// Declare a internal function with C call convention (ccc).
    pub fn declare_internal_c_fn(&self, name: &str, fn_type: &'ll Type) -> &'ll Value {
        let fun = declare_raw_fn(
            self,
            name,
            llvm::CallConv::CCallConv,
            llvm::UnnamedAddr::Global, // Function addresses are never significant, allow functions to be merged
            fn_type,
        );
        unsafe { llvm::LLVMSetLinkage(fun, llvm::Linkage::Internal) }
        fun
    }

    /// Define a a global variable with default external linkage type.
    ///
    /// Returns `None` if the name already has a definition associated with it.
    pub fn define_global(&self, name: &str, ty: &'ll Type) -> Option<&'ll Value> {
        if self.get_defined_global(name).is_none() {
            let name = CString::new(name).unwrap();
            let global = unsafe { llvm::LLVMAddGlobal(self.llmod, ty, name.as_ptr()) };
            Some(global)
        } else {
            None
        }
    }

    /// Declare an unnamed global variable with private linkage.
    pub fn define_private_global(&self, ty: &'ll Type) -> &'ll Value {
        unsafe {
            let global = llvm::LLVMAddGlobal(self.llmod, ty, llvm::UNNAMED);
            llvm::LLVMSetLinkage(global, llvm::Linkage::PrivateLinkage);
            global
        }
    }

    /// Gets global value by name, which could be either a declare or a define.
    pub fn get_global_value(&self, name: &str) -> Option<&'ll Value> {
        let name = CString::new(name).unwrap();
        unsafe { llvm::LLVMGetNamedGlobal(self.llmod, name.as_ptr()) }
    }

    /// Gets defined global value by name.
    pub fn get_defined_global(&self, name: &str) -> Option<&'ll Value> {
        self.get_global_value(name).and_then(|val| {
            let is_decl = unsafe { llvm::LLVMIsDeclaration(val) != llvm::False };
            (!is_decl).then_some(val)
        })
    }

    // JW: not used
    // pub fn define_global_const(&self, ty: &'ll Type, val: &'ll Value) -> &'ll Value {
    //     unsafe {
    //         let res = self.define_private_global(ty);
    //         llvm::LLVMSetInitializer(res, val);
    //         llvm::LLVMSetUnnamedAddress(res, llvm::UnnamedAddr::No);
    //         llvm::LLVMSetGlobalConstant(res, llvm::True);

    //         res
    //     }
    // }

    /// Export a global value with `name` and `ty` and initialize it with `val`.
    pub fn export_val(
        &self,
        name: &str,
        ty: &'ll Type,
        val: &'ll Value,
        is_const: bool,
    ) -> &'ll Value {
        unsafe {
            let res = self
                .define_global(name, ty)
                .unwrap_or_else(|| unreachable!("symbol '{name}' already defined"));

            llvm::LLVMSetInitializer(res, val);
            llvm::LLVMSetLinkage(res, llvm::Linkage::ExternalLinkage);
            llvm::LLVMSetUnnamedAddress(res, llvm::UnnamedAddr::No);
            llvm::LLVMSetDLLStorageClass(res, llvm::DLLStorageClass::Export);
            if is_const {
                llvm::LLVMSetGlobalConstant(res, llvm::True);
            }

            res
        }
    }

    /// Export a global array with `name` and `elem_ty` and initialize it with `vals`.
    pub fn export_array(
        &self,
        name: &str,
        elem_ty: &'ll Type,
        vals: &[&'ll Value],
        is_const: bool,
        add_cnt: bool,
    ) -> &'ll Value {
        let arr = self.export_val(
            name,
            self.ty_array(elem_ty, vals.len() as u32),
            self.const_arr(elem_ty, vals),
            is_const,
        );
        if add_cnt {
            let name = format!("{name}.cnt");
            self.export_val(&name, self.ty_size(), self.const_usize(vals.len()), true);
        }

        arr
    }

    // JW: not used
    // pub fn export_zeroed_array(
    //     &self,
    //     name: &str,
    //     elem_ty: &'ll Type,
    //     len: usize,
    //     add_cnt: bool,
    // ) -> &'ll Value {
    //     let ty = self.ty_array(elem_ty, len as u32);
    //     let arr = self
    //         .define_global(name, ty)
    //         .unwrap_or_else(|| unreachable!("symbol '{name}' already defined"));

    //     unsafe {
    //         let init = llvm::LLVMConstNull(ty);
    //         llvm::LLVMSetInitializer(arr, init);
    //         llvm::LLVMSetLinkage(arr, llvm::Linkage::ExternalLinkage);
    //     }

    //     if add_cnt {
    //         let name = format!("{name}.cnt");
    //         let arr_len = self
    //             .define_global(&name, self.ty_size())
    //             .unwrap_or_else(|| unreachable!("symbol '{name}' already defined"));

    //         unsafe {
    //             let init = self.const_usize(len);
    //             llvm::LLVMSetInitializer(arr_len, init);
    //             llvm::LLVMSetGlobalConstant(arr_len, llvm::True);
    //             llvm::LLVMSetLinkage(arr_len, llvm::Linkage::ExternalLinkage);
    //         }
    //     }

    //     arr
    // }

    pub fn const_arr_ptr(&self, elem_ty: &'ll Type, vals: &[&'ll Value]) -> &'ll Value {
        for (i, val) in vals.iter().enumerate() {
            assert_eq!(
                unsafe { llvm::LLVMTypeOf(val) } as *const Type,
                elem_ty as *const Type,
                "val {i} has mismatched type"
            )
        }

        let ty = self.ty_array(elem_ty, vals.len() as u32);
        let name = self.generate_local_symbol_name("arr");
        let global = self
            .define_global(&name, ty)
            .unwrap_or_else(|| unreachable!("symbol {name} already defined"));

        let val = self.const_arr(elem_ty, vals);
        unsafe {
            llvm::LLVMSetInitializer(global, val);
            llvm::LLVMSetGlobalConstant(global, llvm::True);
            llvm::LLVMSetLinkage(global, llvm::Linkage::Internal);
        }
        global
    }
}
