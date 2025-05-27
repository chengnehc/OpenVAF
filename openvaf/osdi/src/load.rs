use std::iter::zip;

use llvm::{
    LLVMAppendBasicBlockInContext, LLVMBuildCall2, LLVMBuildFAdd, LLVMBuildFDiv, LLVMBuildFMul,
    LLVMBuildFSub, LLVMBuildGEP2, LLVMBuildRetVoid, LLVMBuildStore, LLVMCreateBuilderInContext,
    LLVMDisposeBuilder, LLVMGetParam, LLVMPositionBuilderAtEnd, LLVMSetFastMath,
    LLVMSetPartialFastMath, UNNAMED,
};
use sim_back::dae::NoiseSourceKind;
use typed_index_collections::TiVec;

use crate::compilation_unit::OsdiCompilationUnit;

#[derive(Debug, Clone, Copy)]
pub enum JacobianLoadType {
    Tran,
    Resist,
    React,
}

impl JacobianLoadType {
    const fn dst_reactive(self) -> bool {
        matches!(self, JacobianLoadType::React)
    }

    const fn read_resistive(self) -> bool {
        matches!(self, JacobianLoadType::Resist | JacobianLoadType::Tran)
    }

    const fn read_reactive(self) -> bool {
        matches!(self, JacobianLoadType::React | JacobianLoadType::Tran)
    }

    const fn name(self) -> &'static str {
        match self {
            Self::Tran => "tran",
            Self::Resist => "resist",
            Self::React => "react",
        }
    }
}

impl<'ll> OsdiCompilationUnit<'_, '_, 'll> {
    pub fn load_noise_fn(&self) -> &'ll llvm::Value {
        let OsdiCompilationUnit { cx, module, .. } = self;
        let ptr_t = cx.ty_ptr();
        let fun_ty = cx.ty_func(&[ptr_t, ptr_t, cx.ty_double(), ptr_t], cx.ty_void());
        let name = &format!("load_noise_{}", module.sym);
        let llfunc = cx.declare_internal_c_fn(name, fun_ty);

        unsafe {
            let entry = LLVMAppendBasicBlockInContext(cx.llcx, llfunc, UNNAMED);
            let llbuilder = LLVMCreateBuilderInContext(cx.llcx);
            LLVMPositionBuilderAtEnd(llbuilder, entry);

            let inst = LLVMGetParam(llfunc, 0);
            let model = LLVMGetParam(llfunc, 1);
            let freq = LLVMGetParam(llfunc, 2);
            let dst = LLVMGetParam(llfunc, 3);

            for (i, (src, eval_outputs)) in
                zip(&module.dae.noise_sources, &self.inst_data.noise).enumerate()
            {
                let fac = self.load_eval_output(eval_outputs.factor, inst, model, llbuilder);
                let mut pwr = match src.kind {
                    NoiseSourceKind::WhiteNoise { .. } => {
                        self.load_eval_output(eval_outputs.args[0], inst, model, llbuilder)
                    }
                    NoiseSourceKind::FlickerNoise { .. } => {
                        let mut pwr =
                            self.load_eval_output(eval_outputs.args[0], inst, model, llbuilder);
                        let exp =
                            self.load_eval_output(eval_outputs.args[1], inst, model, llbuilder);
                        let (ty, fun) = self
                            .cx
                            .intrinsic("llvm.pow.f64")
                            .unwrap_or_else(|| unreachable!("intrinsic {name} not found"));
                        let freq_exp =
                            LLVMBuildCall2(llbuilder, ty, fun, [freq, exp].as_ptr(), 2, UNNAMED);
                        LLVMSetPartialFastMath(freq_exp);
                        pwr = LLVMBuildFDiv(llbuilder, pwr, freq_exp, UNNAMED);
                        LLVMSetFastMath(pwr);
                        pwr
                    }
                    NoiseSourceKind::NoiseTable { .. } => unimplemented!("noise tables"),
                };
                // Multiply with squared factor because factor is in terms of signal, but
                // we are computing the power, which is scaled by factor**2.
                pwr = LLVMBuildFMul(llbuilder, pwr, fac, UNNAMED);
                LLVMSetFastMath(pwr);
                pwr = LLVMBuildFMul(llbuilder, pwr, fac, UNNAMED);
                LLVMSetFastMath(pwr);
                let dst = LLVMBuildGEP2(
                    llbuilder,
                    cx.ty_double(),
                    dst,
                    [cx.const_unsigned_int(i as u32)].as_ptr(),
                    1,
                    UNNAMED,
                );
                LLVMBuildStore(llbuilder, pwr, dst);
            }

            LLVMBuildRetVoid(llbuilder);
            LLVMDisposeBuilder(llbuilder);
        }

        llfunc
    }

    pub fn load_residual_fn(&self, reactive: bool) -> &'ll llvm::Value {
        let OsdiCompilationUnit { inst_data, cx, module, .. } = self;
        let ptr_ty = cx.ty_ptr();
        let fun_ty = cx.ty_func(&[ptr_ty; 3], cx.ty_void());
        let name =
            &format!("load_residual_{}_{}", if reactive { "react" } else { "resist" }, module.sym);
        let llfunc = cx.declare_internal_c_fn(name, fun_ty);

        unsafe {
            let entry = LLVMAppendBasicBlockInContext(cx.llcx, llfunc, UNNAMED);
            let llbuilder = LLVMCreateBuilderInContext(cx.llcx);
            LLVMPositionBuilderAtEnd(llbuilder, entry);

            let inst = LLVMGetParam(llfunc, 0);
            let dst = LLVMGetParam(llfunc, 2);

            for node in module.dae.unknowns.indices() {
                if let Some(contrib) = inst_data.load_residual(node, inst, llbuilder, reactive) {
                    inst_data.store_contrib::<false>(cx, node, inst, dst, contrib, llbuilder);
                }
            }

            LLVMBuildRetVoid(llbuilder);
            LLVMDisposeBuilder(llbuilder);
        }

        llfunc
    }

    pub fn load_lim_rhs_fn(&self, reactive: bool) -> &'ll llvm::Value {
        let OsdiCompilationUnit { inst_data, cx, module, .. } = self;
        let ptr_t = cx.ty_ptr();
        let fun_ty = cx.ty_func(&[ptr_t; 3], cx.ty_void());
        let name =
            &format!("load_lim_rhs_{}_{}", if reactive { "react" } else { "resist" }, module.sym);
        let llfunc = cx.declare_internal_c_fn(name, fun_ty);

        unsafe {
            let entry = LLVMAppendBasicBlockInContext(cx.llcx, llfunc, UNNAMED);
            let llbuilder = LLVMCreateBuilderInContext(cx.llcx);
            LLVMPositionBuilderAtEnd(llbuilder, entry);

            let inst = LLVMGetParam(llfunc, 0);
            let dst = LLVMGetParam(llfunc, 2);

            for node in module.dae.unknowns.indices() {
                if let Some(contrib) = inst_data.load_lim_rhs(node, inst, llbuilder, reactive) {
                    inst_data.store_contrib::<true>(cx, node, inst, dst, contrib, llbuilder);
                }
            }

            LLVMBuildRetVoid(llbuilder);
            LLVMDisposeBuilder(llbuilder);
        }

        llfunc
    }

    pub fn load_spice_rhs_<const TRAN: bool>(
        &self,
        llbuilder: &llvm::Builder<'ll>,
        inst: &'ll llvm::Value,
        model: &'ll llvm::Value,
        dst: &'ll llvm::Value,
        prev_solve: &'ll llvm::Value,
        alpha: &'ll llvm::Value,
    ) {
        let dae_system = &self.module.dae;
        let mut node_derivatives = TiVec::from(vec![Vec::new(); dae_system.unknowns.len()]);
        for (id, entry) in dae_system.jacobian.iter_enumerated() {
            node_derivatives[entry.row].push(id)
        }

        unsafe {
            for node in dae_system.unknowns.indices() {
                let mut res = None;
                for &entry in &node_derivatives[node] {
                    let node_deriv = dae_system.jacobian[entry].col;
                    let Some(ddx) = self.load_jacobian_entry::<TRAN>(entry, inst, model, llbuilder)
                    else {
                        continue;
                    };
                    let voltage = self
                        .inst_data
                        .read_node_voltage(self.cx, node_deriv, inst, prev_solve, llbuilder);
                    let val = LLVMBuildFMul(llbuilder, ddx, voltage, UNNAMED);
                    LLVMSetFastMath(val);
                    res = match res {
                        Some(old) => {
                            let val = LLVMBuildFAdd(llbuilder, old, val, UNNAMED);
                            LLVMSetFastMath(val);
                            Some(val)
                        }
                        None => Some(val),
                    }
                }

                let OsdiCompilationUnit { inst_data, cx, .. } = self;
                if !TRAN {
                    if let Some(contrib) = inst_data.load_residual(node, inst, llbuilder, false) {
                        let val = LLVMBuildFSub(
                            llbuilder,
                            res.unwrap_or_else(|| cx.const_real(0.0)),
                            contrib,
                            UNNAMED,
                        );
                        LLVMSetFastMath(val);
                        res = Some(val);
                    }
                }
                if let Some(mut res) = res {
                    if let Some(lim_rhs) = inst_data.load_lim_rhs(node, inst, llbuilder, TRAN) {
                        res = LLVMBuildFAdd(llbuilder, res, lim_rhs, UNNAMED);
                    }
                    if TRAN {
                        res = LLVMBuildFMul(llbuilder, res, alpha, UNNAMED);
                        LLVMSetFastMath(res);
                    }
                    inst_data.store_contrib::<false>(cx, node, inst, dst, res, llbuilder);
                }
            }
        }
    }

    pub fn load_spice_rhs_fn(&self, tran: bool) -> &'ll llvm::Value {
        let OsdiCompilationUnit { cx, module, .. } = self;
        let f64_ty = cx.ty_double();
        let ptr_ty = cx.ty_ptr();
        let mut args = vec![ptr_ty; 4];
        if tran {
            args.push(f64_ty);
        }
        let fun_ty = cx.ty_func(&args, cx.ty_void());
        let name = &format!("load_spice_rhs_{}_{}", if tran { "tran" } else { "dc" }, &module.sym);
        let llfunc = cx.declare_internal_c_fn(name, fun_ty);

        unsafe {
            let entry = LLVMAppendBasicBlockInContext(cx.llcx, llfunc, UNNAMED);
            let llbuilder = LLVMCreateBuilderInContext(cx.llcx);
            LLVMPositionBuilderAtEnd(llbuilder, entry);

            let inst = LLVMGetParam(llfunc, 0);
            let model = LLVMGetParam(llfunc, 1);
            let dst = LLVMGetParam(llfunc, 2);
            let prev_solve = LLVMGetParam(llfunc, 3);
            let alpha = if tran { LLVMGetParam(llfunc, 4) } else { prev_solve };

            self.load_spice_rhs_::<false>(llbuilder, inst, model, dst, prev_solve, alpha);
            if tran {
                self.load_spice_rhs_::<true>(llbuilder, inst, model, dst, prev_solve, alpha);
            }

            LLVMBuildRetVoid(llbuilder);
            LLVMDisposeBuilder(llbuilder);
        }

        llfunc
    }

    pub fn load_jacobian_fn(&self, kind: JacobianLoadType) -> &'ll llvm::Value {
        let OsdiCompilationUnit { cx, module, .. } = *self;
        let args_ = [cx.ty_ptr(), cx.ty_ptr(), cx.ty_double()];
        let args = if kind.read_reactive() { &args_ } else { &args_[0..2] };
        let fun_ty = cx.ty_func(args, cx.ty_void());
        let name = &format!("load_jacobian_{}_{}", kind.name(), &module.sym,);
        let llfunc = cx.declare_internal_c_fn(name, fun_ty);

        unsafe {
            let entry = LLVMAppendBasicBlockInContext(cx.llcx, llfunc, UNNAMED);
            let llbuilder = LLVMCreateBuilderInContext(cx.llcx);

            LLVMPositionBuilderAtEnd(llbuilder, entry);

            let inst = LLVMGetParam(llfunc, 0);
            let model = LLVMGetParam(llfunc, 1);
            let alpha = if kind.read_reactive() { LLVMGetParam(llfunc, 2) } else { inst };

            for entry in module.dae.jacobian.keys() {
                let mut res = if kind.read_resistive() {
                    self.load_jacobian_entry::<false>(entry, inst, model, llbuilder)
                } else {
                    None
                };
                if kind.read_reactive() {
                    if let Some(mut val) =
                        self.load_jacobian_entry::<true>(entry, inst, model, llbuilder)
                    {
                        val = LLVMBuildFMul(llbuilder, val, alpha, UNNAMED);
                        LLVMSetFastMath(val);
                        val = match res {
                            Some(resist) => {
                                let val = LLVMBuildFAdd(llbuilder, resist, val, UNNAMED);
                                LLVMSetFastMath(val);
                                val
                            }
                            None => val,
                        };
                        res = Some(val)
                    }
                }

                if let Some(res) = res {
                    self.inst_data.store_jacobian_contrib(
                        self.cx,
                        entry,
                        inst,
                        llbuilder,
                        kind.dst_reactive(),
                        res,
                    );
                }
            }

            LLVMBuildRetVoid(llbuilder);
            LLVMDisposeBuilder(llbuilder);
        }

        llfunc
    }
}
