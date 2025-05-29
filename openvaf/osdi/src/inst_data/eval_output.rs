use stdx::packed_option::PackedOption;
use stdx::{impl_debug_display, impl_idx_from};

use hir_lower::ParamKind;
use mir::{Const, Function, Param, ValueDef, F_ZERO};
use sim_back::dae;
use typed_indexmap::TiMap;

use super::{strip_optbarrier, CacheSlot, OsdiModule};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct Offset(u32);
impl_idx_from!(Offset(u32));
impl_debug_display!(match Offset{Offset(id) => "offset{id}";});

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum EvalOutput {
    /// This eval output is calculated
    Calculated(EvalOutputSlot),
    /// constant opvar
    Const(Const, PackedOption<EvalOutputSlot>),
    /// model/instance parameter
    Param(Param),
    /// value that was calculated and caches by setup_instance
    Cache(CacheSlot),
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct EvalOutputSlot(u32);
impl_idx_from!(EvalOutputSlot(u32));
impl_debug_display!(match EvalOutputSlot{EvalOutputSlot(id) => "out{id}";});

impl EvalOutput {
    const NONE: EvalOutput = EvalOutput::Cache(CacheSlot(u32::MAX));

    pub fn new<'ll>(
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
