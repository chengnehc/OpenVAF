//! Various utilities used in this crate

use bitset::BitSet;
use hir_lower::HirInterner;
use mir::builder::InstBuilder;
use mir::cursor::{Cursor, FuncCursor};
use mir::{Function, Inst, InstructionData, Opcode, Value, ValueDef, F_ZERO};

/// Is this `val` op-dependent?
pub fn is_op_dependent(
    val: Value,
    func: impl AsRef<Function>,
    intern: &HirInterner,
    op_dependent_insts: &BitSet<Inst>,
) -> bool {
    match func.as_ref().dfg.value_def(val) {
        ValueDef::Result(inst, _) => op_dependent_insts.contains(inst),
        ValueDef::Param(param) => {
            let (kind, _) = intern.params.get_index(param).unwrap();
            kind.is_op_dependent()
        }
        ValueDef::Const(_) | ValueDef::Invalid => false,
    }
}

/// Go back along the use-def chain to get the first actual instruction producing value
pub fn strip_optbarrier(func: &impl AsRef<Function>, mut val: Value) -> Value {
    let func = func.as_ref();
    while let Some(inst) = func.dfg.value_def(val).inst() {
        if let InstructionData::Unary { opcode: Opcode::OptBarrier, arg } = func.dfg.insts[inst] {
            val = arg;
        } else {
            break;
        }
    }
    val
}

pub fn strip_optbarrier_if_const(func: &impl AsRef<Function>, val: Value) -> Value {
    let func = func.as_ref();
    let stripped = strip_optbarrier(func, val);
    if func.dfg.value_def(stripped).as_const().is_some() {
        stripped
    } else {
        val
    }
}

pub fn update_optbarrier(
    func: &mut Function,
    val: &mut Value,
    update: impl FnOnce(Value, &mut FuncCursor) -> Value,
) {
    if let Some(inst) = func.dfg.value_def(*val).inst() {
        // strip a layer of optbarrier, if any
        let stripped = func.dfg.instr_args(inst)[0];
        // update the stripped, original value
        let updated = update(stripped, &mut FuncCursor::new(func).at_inst(inst));
        // replace the optbarrier instruction with the updated value
        func.dfg.replace(inst).optbarrier(updated);
    } else {
        // no existing optbarrier
        let mut cursor = FuncCursor::new(func).at_exit();
        // just update the current value and ensure its optbarrier
        *val = update(*val, &mut cursor);
        *val = cursor.ins().ensure_optbarrier(*val)
    }
}

/// Create instruction that adds or subtracts `val` to/from `dst`.
///
/// `dst` will be set to the result value of this newly created instruction.
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
