use stdx::packed_option::PackedOption;
use stdx::{impl_debug_display, impl_idx_from};

use ahash::RandomState;
use hir::{CompilationDB, ParamSysFun, Parameter, Variable};
use hir_lower::{HirInterner, LimitState, ParamKind, PlaceKind};
use indexmap::IndexMap;
use llvm::{
    IntPredicate, LLVMBuildFAdd, LLVMBuildFSub, LLVMBuildGEP2, LLVMBuildICmp, LLVMBuildIntCast2,
    LLVMBuildLoad2, LLVMBuildStore, LLVMBuildStructGEP2, LLVMConstInt, LLVMOffsetOfElement,
    LLVMSetFastMath, TargetData, UNNAMED,
};
use mir::{strip_optbarrier, Const, Function, Param, ValueDef, F_ZERO};
use mir_llvm::{CodegenCx, MemLoc};
use sim_back::dae::{self, MatrixEntryId, SimUnknown};
use sim_back::init::CacheSlot;
use typed_index_collections::TiVec;
use typed_indexmap::TiMap;

use crate::compilation_unit::{OsdiCompilationUnit, OsdiModule};
use crate::{bitfield, lltype, Offset};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum OsdiInstanceParam {
    Builtin(ParamSysFun),
    User(Parameter),
}

/// Offset of field in OsdiInstanceData struct
pub const NUM_CONST_FIELDS: u32 = 8;
pub const PARAM_GIVEN: u32 = 0;
pub const JACOBIAN_PTR_RESIST: u32 = 1;
pub const JACOBIAN_PTR_REACT: u32 = 2;
pub const NODE_MAPPING: u32 = 3;
pub const COLLAPSED: u32 = 4;
pub const TEMPERATURE: u32 = 5;
pub const CONNECTED: u32 = 6;
pub const STATE_IDX: u32 = 7;

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum EvalOutput {
    /// This eval output is calculated
    Calculated(EvalOutputSlot),
    /// constant opvar
    Const(Const, PackedOption<EvalOutputSlot>),
    /// model/instance parameter
    Param(Param),
    /// ?
    Cache(CacheSlot),
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct EvalOutputSlot(u32);
impl_idx_from!(EvalOutputSlot(u32));
impl_debug_display!(match EvalOutputSlot{EvalOutputSlot(id) => "out{id}";});

impl EvalOutput {
    const NONE: EvalOutput = EvalOutput::Cache(CacheSlot(u32::MAX));

    fn new<'ll>(
        module: &OsdiModule<'_>,
        val: mir::Value,
        ty: &'ll llvm::Type,
        eval_outputs: &mut TiMap<EvalOutputSlot, mir::Value, &'ll llvm::Type>,
        requires_slot: bool, // for const opvars
    ) -> EvalOutput {
        match module.eval.dfg.value_def(val) {
            ValueDef::Result(_, _) => (),
            ValueDef::Param(param) => {
                if let Some((&kind, _)) = module.intern.params.get_index(param) {
                    // parameters are already stored in the model anyway, so no need to create a slot
                    if matches!(
                        kind,
                        ParamKind::Param { .. }
                            | ParamKind::ParamSysFun { .. }
                            | ParamKind::Temperature
                    ) {
                        return EvalOutput::Param(param);
                    }
                } else {
                    let slot = usize::from(param) - module.intern.params.len();
                    return EvalOutput::Cache(slot.into());
                }
            }
            ValueDef::Const(const_val) => {
                let slot = requires_slot.then(|| eval_outputs.insert_full(val, ty).0);
                return EvalOutput::Const(const_val, slot.into());
            }
            ValueDef::Invalid => unreachable!(),
        }

        EvalOutput::Calculated(eval_outputs.insert_full(val, ty).0)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Residual {
    pub resist: PackedOption<EvalOutputSlot>,
    pub react: PackedOption<EvalOutputSlot>,
    pub resist_lim_rhs: PackedOption<EvalOutputSlot>,
    pub react_lim_rhs: PackedOption<EvalOutputSlot>,
}

impl Residual {
    pub fn new<'ll>(
        residual: &dae::Residual,
        slots: &mut TiMap<EvalOutputSlot, mir::Value, &'ll llvm::Type>,
        ty_real: &'ll llvm::Type,
        func: &Function,
    ) -> Residual {
        let mut get_slot = |mut val| {
            val = strip_optbarrier(func, val);
            if val == F_ZERO {
                None.into()
            } else {
                Some(slots.insert_full(val, ty_real).0).into()
            }
        };
        Residual {
            resist: get_slot(residual.resist),
            react: get_slot(residual.react),
            resist_lim_rhs: get_slot(residual.resist_lim_rhs),
            react_lim_rhs: get_slot(residual.react_lim_rhs),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct MatrixEntry {
    pub resist: Option<EvalOutput>,
    pub react: Option<EvalOutput>,
    pub react_off: PackedOption<Offset>,
}

impl MatrixEntry {
    pub fn new<'ll>(
        entry: &dae::MatrixEntry,
        module: &OsdiModule<'_>,
        slots: &mut TiMap<EvalOutputSlot, mir::Value, &'ll llvm::Type>,
        ty_real: &'ll llvm::Type,
        num_react: &mut u32,
    ) -> MatrixEntry {
        let mut get_output = |mut val| {
            val = strip_optbarrier(module.eval, val);
            if val == F_ZERO {
                None
            } else {
                Some(EvalOutput::new(module, val, ty_real, slots, false))
            }
        };
        let react_off = if entry.react == F_ZERO {
            None
        } else {
            *num_react += 1;
            Some(Offset(*num_react - 1))
        };

        MatrixEntry {
            resist: get_output(entry.resist),
            react: get_output(entry.react),
            react_off: react_off.into(),
        }
    }
}

#[derive(Debug)]
pub struct NoiseSource {
    pub factor: EvalOutput,
    /// content of values depend on kind of noise source
    pub args: [EvalOutput; 2],
}

impl NoiseSource {
    pub fn new<'ll>(
        source: &dae::NoiseSource,
        module: &OsdiModule<'_>,
        slots: &mut TiMap<EvalOutputSlot, mir::Value, &'ll llvm::Type>,
        ty_real: &'ll llvm::Type,
    ) -> NoiseSource {
        let mut get_output = |mut val| {
            val = strip_optbarrier(module.eval, val);
            EvalOutput::new(module, val, ty_real, slots, false)
        };
        let args = match source.kind {
            dae::NoiseSourceKind::WhiteNoise { pwr } => [get_output(pwr), EvalOutput::NONE],
            dae::NoiseSourceKind::FlickerNoise { pwr, exp } => [get_output(pwr), get_output(exp)],
            dae::NoiseSourceKind::NoiseTable { .. } => [EvalOutput::NONE; 2],
        };

        NoiseSource { args, factor: get_output(source.factor) }
    }

    pub fn eval_outputs(&self) -> [EvalOutput; 3] {
        [self.factor, self.args[0], self.args[1]]
    }
}

pub struct OsdiInstanceData<'ll> {
    /// llvm type for the instance data struct
    pub ty: &'ll llvm::Type,

    // llvm types for static (always present) instance data struct fields
    pub param_given: &'ll llvm::Type,
    pub jacobian_ptr: &'ll llvm::Type,
    pub jacobian_ptr_react: &'ll llvm::Type,
    pub node_mapping: &'ll llvm::Type,
    pub collapsed: &'ll llvm::Type,
    pub state_idx: &'ll llvm::Type,

    // llvm types for dynamic instance data struct fields
    pub params: IndexMap<OsdiInstanceParam, &'ll llvm::Type, RandomState>,
    // For passing cached op-dependent evaluation results
    pub cache_slots: TiVec<CacheSlot, &'ll llvm::Type>,
    // TODO(JW): what if we combine the store eval_output and load_xxx() these
    // two separate processors into one?
    // i.e., the pointers are passed into the eval() function, after evaluation,
    // eval() will load the results directly to the provided address. After all,
    // when calling eval(), a user implicitly wants the results to be loaded.
    // Then this eval_outputs field is (at least partially) no longer required.
    pub eval_outputs: TiMap<EvalOutputSlot, mir::Value, &'ll llvm::Type>,

    // Not related to any struct fields, but these are all `eval_outputs`
    pub residual: TiVec<SimUnknown, Residual>,
    pub jacobian: TiVec<MatrixEntryId, MatrixEntry>,
    pub opvars: IndexMap<Variable, EvalOutput, RandomState>,
    pub noise: Vec<NoiseSource>,
    pub bound_step: Option<EvalOutputSlot>,
}

impl<'ll> OsdiInstanceData<'ll> {
    pub fn new(db: &CompilationDB, module: &OsdiModule<'_>, cx: &CodegenCx<'_, 'll>) -> Self {
        let f64_t = cx.ty_double();
        let u32_t = cx.ty_int();

        let builtin_inst_params = ParamSysFun::iter().filter_map(|param| {
            let is_live = |intern: &HirInterner, func| {
                intern.is_param_live(func, &ParamKind::ParamSysFun(param))
            };
            let is_live = is_live(module.intern, module.eval)
                || is_live(&module.init.intern, &module.init.func);
            is_live.then_some((OsdiInstanceParam::Builtin(param), f64_t))
        });
        let alias_inst_params = module
            .info
            .param_sysfuns
            .keys()
            .map(|param| (OsdiInstanceParam::Builtin(*param), f64_t));
        let user_inst_params = module
            .info
            .params
            .iter()
            .filter(|&(_param, info)| info.is_instance)
            .map(|(param, _info)| (OsdiInstanceParam::User(*param), lltype(&param.ty(db), cx)));
        let params: IndexMap<_, _, _> =
            builtin_inst_params.chain(alias_inst_params).chain(user_inst_params).collect();

        let cache_slots: TiVec<_, _> =
            module.init.cache_slots.raw.values().map(|ty| lltype(ty, cx)).collect();

        let mut eval_outputs = TiMap::default();
        let opvars = module
            .info
            .op_vars
            .keys()
            .map(|var| {
                let val = module.intern.outputs[&PlaceKind::Var(*var)].unwrap_unchecked();
                let ty = lltype(&var.ty(db), cx);
                let pos = EvalOutput::new(module, val, ty, &mut eval_outputs, true);
                (*var, pos)
            })
            .collect();
        let residual = module
            .dae
            .residual
            .iter()
            .map(|residual| Residual::new(residual, &mut eval_outputs, f64_t, module.eval))
            .collect();
        let mut num_react = 0;
        let jacobian = module
            .dae
            .jacobian
            .iter()
            .map(|entry| MatrixEntry::new(entry, module, &mut eval_outputs, f64_t, &mut num_react))
            .collect();
        let noise = module
            .dae
            .noise_sources
            .iter()
            .map(|source| NoiseSource::new(source, module, &mut eval_outputs, f64_t))
            .collect();

        let bound_step = module.intern.outputs.get(&PlaceKind::BoundStep).and_then(|val| {
            let mut val = val.expand()?;
            val = strip_optbarrier(module.eval, val);
            let slot = eval_outputs.insert_full(val, f64_t).0;
            Some(slot)
        });

        /* Static fields */
        let param_given = bitfield::ty(cx, params.len() as u32);
        let jacobian_ptr = cx.ty_array(cx.ty_ptr(), module.dae.jacobian.len() as u32);
        let jacobian_ptr_react = cx.ty_array(cx.ty_ptr(), num_react);
        let node_mapping = cx.ty_array(u32_t, module.dae.unknowns.len() as u32);
        let collapsed = cx.ty_array(cx.ty_c_bool(), module.node_collapse.num_pairs());
        let temperature = cx.ty_double();
        let connected_ports = cx.ty_int();
        let state_idx = cx.ty_array(cx.ty_int(), module.intern.lim_state.len() as u32);

        let static_fields: [_; NUM_CONST_FIELDS as usize] = [
            param_given,
            jacobian_ptr,
            jacobian_ptr_react,
            node_mapping,
            collapsed,
            temperature,
            connected_ports,
            state_idx,
        ];

        /* Define struct type OsdiInstanceData */
        let name = format!("osdi_inst_data_{}", &module.sym);
        let fields: Vec<_> = static_fields
            .into_iter()
            .chain(params.values().copied())
            .chain(cache_slots.iter().copied())
            .chain(eval_outputs.raw.values().copied())
            .collect();
        let ty = cx.ty_struct(&name, &fields);

        OsdiInstanceData {
            ty,
            param_given,
            jacobian_ptr,
            jacobian_ptr_react,
            node_mapping,
            state_idx,
            collapsed,
            params,
            cache_slots,
            eval_outputs,
            residual,
            jacobian,
            opvars,
            noise,
            bound_step,
        }
    }

    pub unsafe fn is_param_given(
        &self,
        cx: &CodegenCx<'_, 'll>,
        param: OsdiInstanceParam,
        ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> Option<&'ll llvm::Value> {
        let idx = self.params.get_index_of(&param)?;
        let res = self.is_nth_param_given(cx, idx as u32, ptr, llbuilder);
        Some(res)
    }

    pub unsafe fn is_nth_param_given(
        &self,
        cx: &CodegenCx<'_, 'll>,
        idx: u32,
        ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> &'ll llvm::Value {
        let arr_ptr = LLVMBuildStructGEP2(llbuilder, self.ty, ptr, PARAM_GIVEN, UNNAMED);
        bitfield::is_set(cx, idx, arr_ptr, self.param_given, llbuilder)
    }

    pub unsafe fn set_nth_param_given(
        &self,
        cx: &CodegenCx<'_, 'll>,
        idx: u32,
        ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) {
        let arr_ptr = LLVMBuildStructGEP2(llbuilder, self.ty, ptr, PARAM_GIVEN, UNNAMED);
        bitfield::set_bit(cx, idx, arr_ptr, self.param_given, llbuilder)
    }

    // pub unsafe fn set_param_given(
    //     &self,
    //     cx: &CodegenCx<'_, 'll>,
    //     param: OsdiInstanceParam,
    //     ptr: &'ll llvm::Value,
    //     llbuilder: &llvm::Builder<'ll>,
    // ) -> bool {
    //     if let Some(pos) = self.params.get_index_of(&param) {
    //         self.set_nth_param_given(cx, pos as u32, ptr, llbuilder);
    //         true
    //     } else {
    //         false
    //     }
    // }

    pub fn param_loc(
        &self,
        cx: &CodegenCx<'_, 'll>,
        param: OsdiInstanceParam,
        ptr: &'ll llvm::Value,
    ) -> Option<MemLoc<'ll>> {
        let idx = self.params.get_index_of(&param)? as u32;
        let loc = self.nth_param_loc(cx, idx, ptr);
        Some(loc)
    }

    fn nth_param_loc(
        &self,
        cx: &CodegenCx<'_, 'll>,
        idx: u32,
        struct_ptr: &'ll llvm::Value,
    ) -> MemLoc<'ll> {
        let ty = self.params.get_index(idx as usize).unwrap().1;
        let idx = NUM_CONST_FIELDS + idx;
        MemLoc::struct_gep(struct_ptr, self.ty, ty, idx, cx)
    }

    pub unsafe fn param_ptr(
        &self,
        param: OsdiInstanceParam,
        struct_ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> Option<(&'ll llvm::Value, &'ll llvm::Type)> {
        let idx = self.params.get_index_of(&param)? as u32;
        let (ptr, ty) = self.nth_param_ptr(idx, struct_ptr, llbuilder);
        Some((ptr, ty))
    }

    pub unsafe fn nth_param_ptr(
        &self,
        idx: u32,
        struct_ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> (&'ll llvm::Value, &'ll llvm::Type) {
        let (_, ty) = self.params.get_index(idx as usize).unwrap();
        let idx = NUM_CONST_FIELDS + idx;
        let ptr = LLVMBuildStructGEP2(llbuilder, self.ty, struct_ptr, idx, UNNAMED);
        (ptr, ty)
    }

    pub unsafe fn read_param(
        &self,
        param: OsdiInstanceParam,
        struct_ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> Option<&'ll llvm::Value> {
        let (ptr, ty) = self.param_ptr(param, struct_ptr, llbuilder)?;
        let val = LLVMBuildLoad2(llbuilder, ty, ptr, UNNAMED);
        Some(val)
    }

    pub unsafe fn read_nth_param(
        &self,
        idx: u32,
        struct_ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> &'ll llvm::Value {
        let (ptr, ty) = self.nth_param_ptr(idx, struct_ptr, llbuilder);
        LLVMBuildLoad2(llbuilder, ty, ptr, UNNAMED)
    }

    pub unsafe fn store_nth_param(
        &self,
        idx: u32,
        struct_ptr: &'ll llvm::Value,
        val: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> &'ll llvm::Value {
        let (ptr, _) = self.nth_param_ptr(idx, struct_ptr, llbuilder);
        LLVMBuildStore(llbuilder, val, ptr)
    }

    // pub unsafe fn opvar_ptr(
    //     &self,
    //     var: VarId,
    //     ptr: &'ll llvm::Value,
    //     llbuilder: &llvm::Builder<'ll>,
    // ) -> Option<(&'ll llvm::Value, &'ll llvm::Type)> {
    //     let (pos, _, ty) = self.opvars.get_full(&var)?;
    //     let elem = NUM_CONST_FIELDS + self.params.len() as u32 + pos as u32;
    //     let ptr = LLVMBuildStructGEP2(llbuilder, self.ty, ptr, elem, UNNAMED);
    //     Some((ptr, ty))
    // }

    #[inline]
    fn cache_slot_elem(&self, slot: CacheSlot) -> u32 {
        NUM_CONST_FIELDS + self.params.len() as u32 + u32::from(slot)
    }

    fn cache_slot_ptr(
        &self,
        llbuilder: &llvm::Builder<'ll>,
        slot: CacheSlot,
        struct_ptr: &'ll llvm::Value,
    ) -> (&'ll llvm::Value, &'ll llvm::Type) {
        let elem = self.cache_slot_elem(slot);
        let ptr = unsafe { LLVMBuildStructGEP2(llbuilder, self.ty, struct_ptr, elem, UNNAMED) };
        let ty = self.cache_slots[slot];
        (ptr, ty)
    }

    /// Used by eval() to load values cached in op-independent evaluation routines (setup_xxx).
    pub unsafe fn load_cache_slot(
        &self,
        module: &OsdiModule,
        llbuilder: &llvm::Builder<'ll>,
        slot: CacheSlot,
        struct_ptr: &'ll llvm::Value,
    ) -> &'ll llvm::Value {
        let (ptr, ty) = self.cache_slot_ptr(llbuilder, slot, struct_ptr);
        let mut val = LLVMBuildLoad2(llbuilder, ty, ptr, UNNAMED);

        // specifal treatment for bool value
        if module.init.cache_slots[slot] == hir::Type::Bool {
            val = LLVMBuildICmp(
                llbuilder,
                IntPredicate::IntNE,
                val,
                LLVMConstInt(ty, 0, llvm::False),
                UNNAMED,
            );
        }
        val
    }

    /// Used by op-independent evaluation routines (setup_xxx) to cache results,
    /// so that they can be used by op-dependent routines later as parameters.
    pub unsafe fn store_cache_slot(
        &self,
        module: &OsdiModule,
        llbuilder: &llvm::Builder<'ll>,
        slot: CacheSlot,
        ptr: &'ll llvm::Value,
        mut val: &'ll llvm::Value,
    ) {
        let (ptr, ty) = self.cache_slot_ptr(llbuilder, slot, ptr);
        if module.init.cache_slots[slot] == hir::Type::Bool {
            val = LLVMBuildIntCast2(llbuilder, val, ty, llvm::False, UNNAMED);
        }
        LLVMBuildStore(llbuilder, val, ptr);
    }

    pub unsafe fn store_collapsed_node_pair(
        &self,
        cx: &CodegenCx<'_, 'll>,
        llbuilder: &llvm::Builder<'ll>,
        struct_ptr: &'ll llvm::Value,
        pair: &'ll llvm::Value,
    ) {
        let mut ptr = LLVMBuildStructGEP2(llbuilder, self.ty, struct_ptr, COLLAPSED, UNNAMED);
        let zero = cx.const_unsigned_int(0);
        ptr = LLVMBuildGEP2(llbuilder, self.collapsed, ptr, [zero, pair].as_ptr(), 2, UNNAMED);
        LLVMBuildStore(llbuilder, cx.const_c_bool(true), ptr);
    }

    pub unsafe fn temperature_loc(
        &self,
        cx: &CodegenCx<'_, 'll>,
        ptr: &'ll llvm::Value,
    ) -> MemLoc<'ll> {
        MemLoc::struct_gep(ptr, self.ty, cx.ty_double(), TEMPERATURE, cx)
    }

    pub unsafe fn store_temperature(
        &self,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
        struct_ptr: &'ll llvm::Value,
        val: &'ll llvm::Value,
    ) {
        let ptr = builder.struct_gep(self.ty, struct_ptr, TEMPERATURE);
        builder.store(ptr, val)
    }

    pub unsafe fn load_connected_ports(
        &self,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
        struct_ptr: &'ll llvm::Value,
    ) -> &'ll llvm::Value {
        let ptr = builder.struct_gep(self.ty, struct_ptr, CONNECTED);
        builder.load(builder.cx.ty_int(), ptr)
    }

    pub unsafe fn store_connected_ports(
        &self,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
        struct_ptr: &'ll llvm::Value,
        val: &'ll llvm::Value,
    ) {
        let ptr = builder.struct_gep(self.ty, struct_ptr, CONNECTED);
        builder.store(ptr, val)
    }

    pub fn residual_off(
        &self,
        node: SimUnknown,
        reactive: bool,
        target_data: &TargetData,
    ) -> Option<u32> {
        let residual = &self.residual[node];
        let output_slot = if reactive { &residual.react } else { &residual.resist };
        let elem = NUM_CONST_FIELDS
            + self.params.len() as u32
            + self.cache_slots.len() as u32
            + u32::from(output_slot.expand()?);

        let off = unsafe { LLVMOffsetOfElement(target_data, self.ty, elem) } as u32;
        Some(off)
    }

    pub fn lim_rhs_off(
        &self,
        node: SimUnknown,
        reactive: bool,
        target_data: &TargetData,
    ) -> Option<u32> {
        let residual = &self.residual[node];
        let residual = if reactive { &residual.react_lim_rhs } else { &residual.resist_lim_rhs };
        let slot = residual.expand()?;
        let elem = self.eval_output_slot_elem(slot);
        let off = unsafe { LLVMOffsetOfElement(target_data, self.ty, elem) } as u32;
        Some(off)
    }

    unsafe fn eval_output_slot_ptr(
        &self,
        llbuilder: &llvm::Builder<'ll>,
        struct_ptr: &'ll llvm::Value,
        slot: EvalOutputSlot,
    ) -> (&'ll llvm::Value, &'ll llvm::Type) {
        let elem = self.eval_output_slot_elem(slot);
        let ptr = LLVMBuildStructGEP2(llbuilder, self.ty, struct_ptr, elem, UNNAMED);
        let ty = self.eval_outputs.get_index(slot).unwrap().1;
        (ptr, ty)
    }

    #[inline]
    fn eval_output_slot_elem(&self, slot: EvalOutputSlot) -> u32 {
        NUM_CONST_FIELDS
            + self.params.len() as u32
            + self.cache_slots.len() as u32
            + u32::from(slot)
    }

    unsafe fn load_eval_output_slot(
        &self,
        llbuilder: &llvm::Builder<'ll>,
        struct_ptr: &'ll llvm::Value,
        slot: EvalOutputSlot,
    ) -> &'ll llvm::Value {
        let (ptr, ty) = self.eval_output_slot_ptr(llbuilder, struct_ptr, slot);
        LLVMBuildLoad2(llbuilder, ty, ptr, UNNAMED)
    }

    unsafe fn store_eval_output_slot(
        &self,
        slot: EvalOutputSlot,
        inst_ptr: &'ll llvm::Value,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
    ) {
        let val = *self.eval_outputs.get_index(slot).unwrap().0;
        let val = builder.values[val].get(builder);
        let (ptr, _) = self.eval_output_slot_ptr(builder.llbuilder, inst_ptr, slot);
        builder.store(ptr, val)
    }

    /// Used by eval() to store its output
    pub unsafe fn store_eval_output(
        &self,
        output: EvalOutput,
        inst_ptr: &'ll llvm::Value,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
    ) {
        if let EvalOutput::Calculated(slot) = output {
            self.store_eval_output_slot(slot, inst_ptr, builder)
        }
    }

    /// Get node offset defined by node mapping
    pub unsafe fn node_off(
        &self,
        cx: &CodegenCx<'_, 'll>,
        node: SimUnknown,
        struct_ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> &'ll llvm::Value {
        let ptr = LLVMBuildStructGEP2(llbuilder, self.ty, struct_ptr, NODE_MAPPING, UNNAMED);
        let zero = cx.const_int(0);
        let node = cx.const_unsigned_int(node.into());
        let ptr =
            LLVMBuildGEP2(llbuilder, self.node_mapping, ptr, [zero, node].as_ptr(), 2, UNNAMED);
        LLVMBuildLoad2(llbuilder, cx.ty_int(), ptr, UNNAMED)
    }

    /// Read the node offset first, and then read node voltage at this
    /// offset in the solution vector
    pub unsafe fn read_node_voltage(
        &self,
        cx: &CodegenCx<'_, 'll>,
        node: SimUnknown,
        ptr: &'ll llvm::Value,
        prev_solve: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> &'ll llvm::Value {
        let off = self.node_off(cx, node, ptr, llbuilder);
        let ptr = LLVMBuildGEP2(llbuilder, cx.ty_double(), prev_solve, [off].as_ptr(), 1, UNNAMED);
        LLVMBuildLoad2(llbuilder, cx.ty_double(), ptr, UNNAMED)
    }

    /// Used by load_xxx()
    pub unsafe fn load_residual(
        &self,
        node: SimUnknown,
        ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
        reactive: bool,
    ) -> Option<&'ll llvm::Value> {
        let residual = &self.residual[node];
        let residual = if reactive { &residual.react } else { &residual.resist };
        let val = self.load_eval_output_slot(llbuilder, ptr, residual.expand()?);
        Some(val)
    }

    pub unsafe fn load_lim_rhs(
        &self,
        node: SimUnknown,
        ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
        reactive: bool,
    ) -> Option<&'ll llvm::Value> {
        let residual = &self.residual[node];
        let lim_rhs = if reactive { &residual.react_lim_rhs } else { &residual.resist_lim_rhs };
        let val = self.load_eval_output_slot(llbuilder, ptr, lim_rhs.expand()?);
        Some(val)
    }

    /// Used by eval()
    pub unsafe fn read_state_idx(
        &self,
        cx: &CodegenCx<'_, 'll>,
        idx: LimitState,
        ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> &'ll llvm::Value {
        let ptr = LLVMBuildStructGEP2(llbuilder, self.ty, ptr, STATE_IDX, UNNAMED);
        let zero = cx.const_int(0);
        let state = cx.const_unsigned_int(idx.into());
        let ptr = LLVMBuildGEP2(llbuilder, self.state_idx, ptr, [zero, state].as_ptr(), 2, UNNAMED);
        LLVMBuildLoad2(llbuilder, cx.ty_int(), ptr, UNNAMED)
    }

    /// Used by eval()
    pub unsafe fn store_residual(
        &self,
        node: SimUnknown,
        ptr: &'ll llvm::Value,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
        reactive: bool,
    ) -> bool {
        let residual = &self.residual[node];
        let slot = if reactive { residual.react } else { residual.resist };
        if let Some(slot) = slot.expand() {
            self.store_eval_output_slot(slot, ptr, builder);
            true
        } else {
            false
        }
    }

    /// Used by load_xxx() to store residual to dst
    pub unsafe fn store_contrib<const NEG: bool>(
        &self,
        cx: &CodegenCx<'_, 'll>,
        node: SimUnknown,
        ptr: &'ll llvm::Value,
        dst: &'ll llvm::Value,
        contrib: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) {
        let off = self.node_off(cx, node, ptr, llbuilder);
        let dst = LLVMBuildGEP2(llbuilder, cx.ty_double(), dst, [off].as_ptr(), 1, UNNAMED);
        let old = LLVMBuildLoad2(llbuilder, cx.ty_double(), dst, UNNAMED);
        let val = if NEG {
            LLVMBuildFSub(llbuilder, old, contrib, UNNAMED)
        } else {
            LLVMBuildFAdd(llbuilder, old, contrib, UNNAMED)
        };
        LLVMSetFastMath(val);
        LLVMBuildStore(llbuilder, val, dst);
    }

    pub unsafe fn store_lim_rhs(
        &self,
        node: SimUnknown,
        ptr: &'ll llvm::Value,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
        reactive: bool,
    ) -> bool {
        let dst = &self.residual[node];
        let slot = if reactive { dst.react_lim_rhs } else { dst.resist_lim_rhs };
        if let Some(slot) = slot.expand() {
            self.store_eval_output_slot(slot, ptr, builder);
            true
        } else {
            false
        }
    }

    /// Used by eval() to store its output Jacobian entry
    pub unsafe fn store_jacobian(
        &self,
        entry: MatrixEntryId,
        inst_ptr: &'ll llvm::Value,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
        reactive: bool,
    ) {
        let entry = &self.jacobian[entry];
        let dst = if reactive { entry.react } else { entry.resist };
        if let Some(EvalOutput::Calculated(slot)) = dst {
            self.store_eval_output_slot(slot, inst_ptr, builder)
        }
    }

    /// Used by load_xxx()
    pub unsafe fn store_jacobian_contrib(
        &self,
        cx: &CodegenCx<'_, 'll>,
        entry: MatrixEntryId,
        ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
        reactive: bool,
        val: &'ll llvm::Value,
    ) {
        let zero = cx.const_int(0);
        let field = if reactive { JACOBIAN_PTR_REACT } else { JACOBIAN_PTR_RESIST };
        let ty = if reactive { self.jacobian_ptr_react } else { self.jacobian_ptr };
        let ptr = LLVMBuildStructGEP2(llbuilder, self.ty, ptr, field, UNNAMED);
        let entry = if reactive {
            self.jacobian[entry].react_off.unwrap_unchecked().into()
        } else {
            entry.into()
        };
        let entry = cx.const_unsigned_int(entry);
        let ptr = LLVMBuildGEP2(llbuilder, ty, ptr, [zero, entry].as_ptr(), 2, UNNAMED);
        let dst = LLVMBuildLoad2(llbuilder, cx.ty_ptr(), ptr, UNNAMED);
        let old = LLVMBuildLoad2(llbuilder, cx.ty_double(), dst, UNNAMED);
        let val = LLVMBuildFAdd(llbuilder, old, val, UNNAMED);
        LLVMSetFastMath(val);
        LLVMBuildStore(llbuilder, val, dst);
    }

    pub unsafe fn store_bound_step(
        &self,
        ptr: &'ll llvm::Value,
        builder: &mir_llvm::Builder<'_, '_, 'll>,
    ) {
        if let Some(slot) = self.bound_step {
            self.store_eval_output_slot(slot, ptr, builder);
        }
    }

    pub fn bound_step_elem(&self) -> Option<u32> {
        let elem = self.eval_output_slot_elem(self.bound_step?);
        Some(elem)
    }
}

impl<'ll> OsdiCompilationUnit<'_, '_, 'll> {
    pub unsafe fn load_eval_output(
        &self,
        output: EvalOutput,
        inst_ptr: &'ll llvm::Value,
        model_ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> &'ll llvm::Value {
        let OsdiCompilationUnit { inst_data, model_data, cx, module, .. } = self;
        let (ptr, ty) = match output {
            EvalOutput::Calculated(slot) => {
                inst_data.eval_output_slot_ptr(llbuilder, inst_ptr, slot)
            }
            EvalOutput::Const(val, _) => {
                return cx.const_val(&val);
            }
            EvalOutput::Param(param) => {
                let intern = &module.intern;
                let (kind, _) = intern.params.get_index(param).unwrap();
                match *kind {
                    ParamKind::Param(param) => inst_data
                        .param_ptr(OsdiInstanceParam::User(param), inst_ptr, llbuilder)
                        .unwrap_or_else(|| {
                            model_data.param_ptr(param, model_ptr, llbuilder).unwrap()
                        }),
                    ParamKind::Temperature => (
                        LLVMBuildStructGEP2(
                            llbuilder,
                            cx.ty_double(),
                            inst_ptr,
                            TEMPERATURE,
                            UNNAMED,
                        ),
                        cx.ty_double(),
                    ),
                    ParamKind::ParamSysFun(func) => inst_data
                        .param_ptr(OsdiInstanceParam::Builtin(func), inst_ptr, llbuilder)
                        .unwrap(),

                    ParamKind::HiddenState(_) => todo!("hidden state"),

                    ParamKind::Voltage { .. }
                    | ParamKind::Current(_)
                    | ParamKind::PortConnected { .. }
                    | ParamKind::ParamGiven { .. }
                    | ParamKind::Abstime
                    | ParamKind::EnableIntegration
                    | ParamKind::EnableLim
                    | ParamKind::PrevState(_)
                    | ParamKind::NewState(_)
                    | ParamKind::ImplicitUnknown(_) => unreachable!(),
                }
            }
            EvalOutput::Cache(slot) => inst_data.cache_slot_ptr(llbuilder, slot, inst_ptr),
        };

        LLVMBuildLoad2(llbuilder, ty, ptr, UNNAMED)
    }

    pub unsafe fn load_jacobian_entry<const REACTIVE: bool>(
        &self,
        entry: MatrixEntryId,
        inst_ptr: &'ll llvm::Value,
        model_ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> Option<&'ll llvm::Value> {
        let entry = &self.inst_data.jacobian[entry];
        let entry = if REACTIVE { entry.react } else { entry.resist };
        let val = self.load_eval_output(entry?, inst_ptr, model_ptr, llbuilder);
        Some(val)
    }

    pub unsafe fn nth_opvar_ptr(
        &self,
        idx: u32,
        inst_ptr: &'ll llvm::Value,
        model_ptr: &'ll llvm::Value,
        llbuilder: &llvm::Builder<'ll>,
    ) -> (&'ll llvm::Value, &'ll llvm::Type) {
        let OsdiCompilationUnit { inst_data, model_data, cx, module, .. } = self;
        match *inst_data.opvars.get_index(idx as usize).unwrap().1 {
            EvalOutput::Calculated(slot) => {
                inst_data.eval_output_slot_ptr(llbuilder, inst_ptr, slot)
            }
            EvalOutput::Const(val, slot) => {
                let (ptr, ty) = inst_data.eval_output_slot_ptr(llbuilder, inst_ptr, slot.unwrap());
                LLVMBuildStore(llbuilder, cx.const_val(&val), ptr);
                (ptr, ty)
            }
            EvalOutput::Param(param) => {
                let intern = &module.intern;
                let (kind, _) = intern.params.get_index(param).unwrap();
                match *kind {
                    ParamKind::Param(param) => inst_data
                        .param_ptr(OsdiInstanceParam::User(param), inst_ptr, llbuilder)
                        .unwrap_or_else(|| {
                            model_data.param_ptr(param, model_ptr, llbuilder).unwrap()
                        }),
                    ParamKind::Temperature => (
                        LLVMBuildStructGEP2(
                            llbuilder,
                            cx.ty_double(),
                            inst_ptr,
                            TEMPERATURE,
                            UNNAMED,
                        ),
                        cx.ty_double(),
                    ),
                    ParamKind::ParamSysFun(func) => inst_data
                        .param_ptr(OsdiInstanceParam::Builtin(func), inst_ptr, llbuilder)
                        .unwrap(),

                    ParamKind::HiddenState(_) => todo!("hidden state"),

                    ParamKind::Voltage { .. }
                    | ParamKind::Current(_)
                    | ParamKind::PortConnected { .. }
                    | ParamKind::ParamGiven { .. }
                    | ParamKind::EnableIntegration
                    | ParamKind::Abstime
                    | ParamKind::EnableLim
                    | ParamKind::PrevState(_)
                    | ParamKind::NewState(_)
                    | ParamKind::ImplicitUnknown(_) => unreachable!(),
                }
            }
            EvalOutput::Cache(slot) => inst_data.cache_slot_ptr(llbuilder, slot, inst_ptr),
        }
    }
}
