//! various utilities used in this crate

use bitset::BitSet;
use hir_lower::HirInterner;
use mir::builder::InstBuilder;
use mir::cursor::{Cursor, FuncCursor};
use mir::{strip_optbarrier, Function, Inst, Value, ValueDef, F_ZERO};

pub fn strip_optbarrier_if_const(func: impl AsRef<Function>, val: Value) -> Value {
    let func = func.as_ref();
    let stripped = strip_optbarrier(func, val);
    if func.dfg.value_def(stripped).as_const().is_some() {
        stripped
    } else {
        val
    }
}

pub fn is_op_dependent(
    func: impl AsRef<Function>,
    val: Value,
    op_dependent_insts: &BitSet<Inst>,
    intern: &HirInterner,
) -> bool {
    match func.as_ref().dfg.value_def(val) {
        ValueDef::Result(inst, _) => op_dependent_insts.contains(inst),
        ValueDef::Param(param) => intern.params.get_index(param).unwrap().0.is_op_dependent(),
        ValueDef::Const(_) | ValueDef::Invalid => false,
    }
}

pub fn update_optbarrier(
    func: &mut Function,
    val: &mut Value,
    update: impl FnOnce(Value, &mut FuncCursor) -> Value,
) {
    if let Some(inst) = func.dfg.value_def(*val).inst() {
        let mut arg = func.dfg.instr_args(inst)[0];
        arg = update(arg, &mut FuncCursor::new(func).at_inst(inst));
        func.dfg.replace(inst).optbarrier(arg);
    } else {
        let mut cursor = FuncCursor::new(&mut *func).at_exit();
        *val = update(*val, &mut cursor);
        *val = cursor.ins().ensure_optbarrier(*val)
    }
}

/// Create MIR instruction that adds or subtracts `val` to `dst`.
///
/// Sets dst to this new value.
pub fn add(cursor: &mut FuncCursor, dst: &mut Value, val: Value, negate: bool) {
    match (*dst, val) {
        // val is zero, do nothing
        (_, F_ZERO) => (),

        // dst is zero, with negation, create "fneg val"
        (F_ZERO, _) if negate => *dst = cursor.ins().fneg(val),

        // dst is zero, without negation, create optbarrier,
        // otherwise a node with only one voltage noise contribution will produce a singular Jacobian.
        // The KCL entry of the node in the Jacobian will be missing the branch current contribution.
        // Refer to: https://github.com/pascalkuthe/OpenVAF/issues/135
        //
        // buggy code:
        // (F_ZERO, _) => *dst = val,
        (F_ZERO, _) => *dst = cursor.ins().optbarrier(val),

        // negate, create "fsub dst, val"
        (old, _) if negate => *dst = cursor.ins().fsub(old, val),

        // do not negate, create "fadd dst, val"
        (old, _) => *dst = cursor.ins().fadd(old, val),
    }
}
