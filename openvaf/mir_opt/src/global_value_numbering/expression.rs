use std::hash::{BuildHasher, Hash, Hasher};
use std::mem::{swap, ManuallyDrop};
use stdx::packed_option::PackedOption;

use ahash::RandomState;
use mir::{Block, FuncRef, Function, Inst, InstructionData, Opcode, Value, ValueDef, ValueList};

use super::{ClassId, GVN};
use crate::simplify::SimplifyCtx;

pub(crate) enum ExprResult {
    Expr(GVNExpression),
    Simplified(ClassId),
}

impl ExprResult {
    /// Turn the expression result into a class
    pub(super) fn into_class(self, inst: Inst, gvn: &mut GVN, func: &mut Function) -> ClassId {
        match self {
            ExprResult::Expr(expr) => gvn.class_map.insert_expr(inst, expr, func),
            ExprResult::Simplified(class) => class,
        }
    }

    /// Generate the expression id of `inst`, which is either create a new GVNExpression
    /// or reuse the previous simplified one.
    ///
    /// Returns `None` if the instruction is:
    /// - optbarrier
    /// - branch or jump
    /// - callback with side-effects
    pub(super) fn from_inst(inst: Inst, gvn: &GVN, func: &mut Function) -> Option<ExprResult> {
        let (opcode, payload) = match func.dfg.insts[inst].clone() {
            InstructionData::Unary { opcode, mut arg } if opcode != Opcode::OptBarrier => {
                arg = gvn.get_lead_val(arg, func);

                let simplified_val = gvn.simplify_ctx(func).simplify_unary_op(opcode, arg);
                if let Some(val) = simplified_val {
                    if let Some(expr) = gvn.check_simplified(val, func) {
                        return Some(expr);
                    }
                }
                (opcode, PayLoad { default: DefaultPayLoad { val1: arg, val2: None.into() } })
            }
            InstructionData::Binary { opcode, args: [mut val1, mut val2] } => {
                val1 = gvn.get_lead_val(val1, func);
                val2 = gvn.get_lead_val(val2, func);
                if opcode.is_commutative() && gvn.should_swap_operands(val1, val2, func) {
                    swap(&mut val1, &mut val2);
                }

                let simplified_val = gvn.simplify_ctx(func).simplify_binop(opcode, val1, val2);
                if let Some(val) = simplified_val {
                    if let Some(expr) = gvn.check_simplified(val, func) {
                        return Some(expr);
                    }
                }
                (opcode, PayLoad { default: DefaultPayLoad { val1, val2: val2.into() } })
            }
            InstructionData::PhiNode(phi) => {
                let simplified_val = gvn.simplify_ctx(func).simplify_phi(phi.clone());
                if let Some(val) = simplified_val {
                    if let Some(expr) = gvn.check_simplified(val, func) {
                        return Some(expr);
                    }
                }

                let bb = func.layout.inst_block(inst).unwrap();
                let mut args = ValueList::new();
                for (_, i) in phi.blocks.iter(&func.dfg.phi_forest) {
                    let pool = &mut func.dfg.insts.value_lists;
                    let val = phi.args.get(i as usize, pool).unwrap();
                    let val = gvn.class_map.get_lead_val(val, func);
                    let pool = &mut func.dfg.insts.value_lists;
                    args.push(val, pool);
                }
                (Opcode::Phi, PayLoad { phi: ManuallyDrop::new(PhiPayLoad { bb, args }) })
            }
            InstructionData::Call { func_ref, args }
                if !func.dfg.signatures[func_ref].has_side_effects =>
            {
                let mut args = args.deep_clone(&mut func.dfg.insts.value_lists);
                for i in 0..args.len(&func.dfg.insts.value_lists) {
                    let arg =
                        unsafe { args.get(i, &func.dfg.insts.value_lists).unwrap_unchecked() };
                    let arg = gvn.class_map.get_lead_val(arg, func);
                    unsafe {
                        *args.get_mut(i, &mut func.dfg.insts.value_lists).unwrap_unchecked() = arg;
                    };
                }
                (Opcode::Call, PayLoad { call: ManuallyDrop::new(CallPayLoad { func_ref, args }) })
            }

            _ => return None,
        };

        Some(ExprResult::Expr(GVNExpression { opcode, payload }))
    }
}

impl GVN {
    fn simplify_ctx<'a>(
        &'a self,
        func: &'a mut Function,
    ) -> SimplifyCtx<'a, f64, impl Fn(Value, &Function) -> Value + 'a> {
        SimplifyCtx::new(func, |val, func| self.class_map.get_lead_val(val, func))
    }

    fn check_simplified(&self, val: Value, func: &Function) -> Option<ExprResult> {
        match func.dfg.value_def(val) {
            // the value is simplified to a previous instruction result:
            // attach an existing GVN expression class ID to it.
            ValueDef::Result(inst, _) => {
                self.class_map.inst_class[inst].expand().map(ExprResult::Simplified)
            }
            // the value is simplified to a parameter or a constant:
            // create a dummy GVN expression class for it.
            // can't get any better than this ;)
            ValueDef::Param(_) | ValueDef::Const(_) => {
                Some(ExprResult::Expr(GVNExpression::new_const(val)))
            }
            ValueDef::Invalid => unreachable!(),
        }
    }

    /// Get the leading value that is semantically equivalent to `val`.
    fn get_lead_val(&self, val: Value, func: &Function) -> Value {
        self.class_map.get_lead_val(val, func)
    }

    /// Should the two operand value of a commutative binary instruction be swapped,
    /// according to the rank computed by GVN?
    fn should_swap_operands(&self, val1: Value, val2: Value, func: &Function) -> bool {
        self.dfs_map.should_swap(val1, val2, func)
    }
}

pub(crate) struct GVNExpression {
    pub(super) opcode: Opcode,
    pub(super) payload: PayLoad,
}

/* Not used */
impl Clone for GVNExpression {
    fn clone(&self) -> Self {
        let payload = unsafe { std::ptr::read(&self.payload) };
        Self { opcode: self.opcode, payload }
    }
}

impl GVNExpression {
    // As we don't value number optbarrier instructions, this is a nice optimization.
    fn new_const(val: Value) -> GVNExpression {
        GVNExpression {
            opcode: Opcode::OptBarrier,
            payload: PayLoad { default: DefaultPayLoad { val1: val, val2: None.into() } },
        }
    }

    /// Dispose the GVN expression and its payload.
    pub(super) fn destroy(&mut self, func: &mut Function) {
        match self.opcode {
            Opcode::Phi => self.payload.phi_mut().args.clear(&mut func.dfg.insts.value_lists),
            Opcode::Call => self.payload.call_mut().args.clear(&mut func.dfg.insts.value_lists),
            _ => (),
        }
    }

    /// Produce a hash of this GVN expression.
    pub(super) fn hash(&self, state: &RandomState, func: &Function) -> u64 {
        let mut hasher = state.build_hasher();
        self.opcode.hash(&mut hasher);
        match self.opcode {
            Opcode::Phi => {
                let PhiPayLoad { bb, args } = self.payload.phi();
                bb.hash(&mut hasher);
                for arg in args.as_slice(&func.dfg.insts.value_lists) {
                    arg.hash(&mut hasher)
                }
            }
            Opcode::Call => {
                let CallPayLoad { func_ref, args } = self.payload.call();
                func_ref.hash(&mut hasher);
                for arg in args.as_slice(&func.dfg.insts.value_lists) {
                    arg.hash(&mut hasher)
                }
            }
            _ => {
                let DefaultPayLoad { val1, val2 } = self.payload.default();
                val1.hash(&mut hasher);
                val2.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Test if this expression is equivalent to `other`.
    pub(super) fn eq(&self, other: &Self, func: &Function) -> bool {
        if self.opcode != other.opcode {
            return false;
        }

        match self.opcode {
            Opcode::Phi => {
                let PhiPayLoad { bb: bb1, args: args1 } = self.payload.phi();
                let PhiPayLoad { bb: bb2, args: args2 } = other.payload.phi();
                if bb1 != bb2 {
                    return false;
                }
                let args1 = args1.as_slice(&func.dfg.insts.value_lists);
                let args2 = args2.as_slice(&func.dfg.insts.value_lists);
                args1.iter().eq(args2)
            }
            Opcode::Call => {
                let CallPayLoad { func_ref: func_ref_1, args: args1 } = self.payload.call();
                // JW: fixed from `self.payload` to `other.payload`, a typo that leads to weird test fails :(
                let CallPayLoad { func_ref: func_ref_2, args: args2 } = other.payload.call();
                if func_ref_1 != func_ref_2 {
                    return false;
                }
                let args1 = args1.as_slice(&func.dfg.insts.value_lists);
                let args2 = args2.as_slice(&func.dfg.insts.value_lists);
                args1.iter().eq(args2)
            }
            _ => {
                let payload1 = self.payload.default();
                let payload2 = other.payload.default();
                payload1 == payload2
            }
        }
    }
}

pub(super) union PayLoad {
    default: DefaultPayLoad,
    // this is basically copy but that just makes the API of `ValueList` really awkward
    phi: ManuallyDrop<PhiPayLoad>,
    call: ManuallyDrop<CallPayLoad>,
}

impl PayLoad {
    pub(super) fn default(&self) -> DefaultPayLoad {
        unsafe { self.default }
    }
    fn phi(&self) -> &PhiPayLoad {
        unsafe { &self.phi }
    }
    fn phi_mut(&mut self) -> &mut PhiPayLoad {
        unsafe { &mut self.phi }
    }
    fn call(&self) -> &CallPayLoad {
        unsafe { &self.call }
    }
    fn call_mut(&mut self) -> &mut CallPayLoad {
        unsafe { &mut self.call }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DefaultPayLoad {
    // We do not have operation using 3 arguments so this is fine.
    pub(super) val1: Value,
    pub(super) val2: PackedOption<Value>,
}

#[derive(Clone, Debug)]
struct PhiPayLoad {
    /// The basic block where the phi resides in, not block parameters.
    bb: Block,
    args: ValueList,
}

#[derive(Clone, Debug)]
struct CallPayLoad {
    func_ref: FuncRef,
    args: ValueList,
}
