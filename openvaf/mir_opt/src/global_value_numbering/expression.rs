use std::hash::{BuildHasher, Hash, Hasher};
use std::mem::{swap, ManuallyDrop};
use stdx::packed_option::PackedOption;

use ahash::RandomState;
use mir::{Block, FuncRef, Function, Inst, InstructionData, Opcode, Value, ValueList};

use super::{ClassId, GVN};

pub(crate) enum ExprResult {
    Expr(GVNExpression),
    Simplified(ClassId),
}

impl ExprResult {
    pub(super) fn into_class(self, inst: Inst, gvn: &mut GVN, func: &mut Function) -> ClassId {
        match self {
            ExprResult::Expr(expr) => gvn.class_map.insert_expr(inst, expr, func),
            ExprResult::Simplified(class) => class,
        }
    }
}

pub(crate) struct GVNExpression {
    pub(super) opcode: Opcode,
    pub(super) payload: GVNExprPayLoad,
}

impl Clone for GVNExpression {
    fn clone(&self) -> Self {
        let payload = unsafe { std::ptr::read(&self.payload) };
        Self { opcode: self.opcode, payload }
    }
}

impl GVNExpression {
    #[allow(clippy::new_ret_no_self)]
    pub fn new(gvn: &mut GVN, func: &mut Function, inst: Inst) -> Option<ExprResult> {
        let (opcode, payload) = match func.dfg.insts[inst].clone() {
            InstructionData::Unary { opcode, mut arg } if opcode != Opcode::OptBarrier => {
                arg = gvn.get_lead_val(arg, func);

                let simplified_val = gvn.simplify_ctx(func).simplify_unary_op(opcode, arg);
                if let Some(val) = simplified_val {
                    if let Some(expr) = gvn.check_simplified(val, func) {
                        return Some(expr);
                    }
                }
                (
                    opcode,
                    GVNExprPayLoad { default: DefaultExprPayLoad { val1: arg, val2: None.into() } },
                )
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
                (opcode, GVNExprPayLoad { default: DefaultExprPayLoad { val1, val2: val2.into() } })
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
                (
                    Opcode::Phi,
                    GVNExprPayLoad { phi: ManuallyDrop::new(PhiExprPayLoad { bb, args }) },
                )
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
                (
                    Opcode::Call,
                    GVNExprPayLoad { call: ManuallyDrop::new(CallExprPayLoad { func_ref, args }) },
                )
            }

            _ => return None,
        };

        Some(ExprResult::Expr(GVNExpression { opcode, payload }))
    }

    pub fn new_const(val: Value) -> GVNExpression {
        GVNExpression {
            // we don't value number optbarrier instructions so this is a nice niece optimization
            opcode: Opcode::OptBarrier,
            payload: GVNExprPayLoad {
                default: DefaultExprPayLoad { val1: val, val2: None.into() },
            },
        }
    }

    /// Produce a hash of this GVN expression.
    pub(super) fn hash(&self, state: &RandomState, func: &Function) -> u64 {
        let mut hasher = state.build_hasher();
        self.opcode.hash(&mut hasher);
        match self.opcode {
            Opcode::Phi => {
                let PhiExprPayLoad { bb, args } = self.payload.phi();
                bb.hash(&mut hasher);
                for arg in args.as_slice(&func.dfg.insts.value_lists) {
                    arg.hash(&mut hasher)
                }
            }
            Opcode::Call => {
                let CallExprPayLoad { func_ref, args } = self.payload.call();
                func_ref.hash(&mut hasher);
                for arg in args.as_slice(&func.dfg.insts.value_lists) {
                    arg.hash(&mut hasher)
                }
            }
            _ => {
                let DefaultExprPayLoad { val1, val2 } = self.payload.default();
                val1.hash(&mut hasher);
                val2.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    pub(super) fn eq(&self, other: &Self, func: &Function) -> bool {
        if self.opcode != other.opcode {
            return false;
        }

        match self.opcode {
            Opcode::Phi => {
                let PhiExprPayLoad { bb: bb1, args: args1 } = self.payload.phi();
                let PhiExprPayLoad { bb: bb2, args: args2 } = other.payload.phi();
                if bb1 != bb2 {
                    return false;
                }
                let args1 = args1.as_slice(&func.dfg.insts.value_lists);
                let args2 = args2.as_slice(&func.dfg.insts.value_lists);
                args1.iter().eq(args2)
            }
            Opcode::Call => {
                let CallExprPayLoad { func_ref: func_ref_1, args: args1 } = self.payload.call();
                let CallExprPayLoad { func_ref: func_ref_2, args: args2 } = self.payload.call();
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

    pub(super) fn destroy(&mut self, func: &mut Function) {
        match self.opcode {
            Opcode::Phi => {
                self.payload.phi_mut().args.clear(&mut func.dfg.insts.value_lists);
            }
            Opcode::Call => {
                self.payload.call_mut().args.clear(&mut func.dfg.insts.value_lists);
            }
            _ => (),
        }
    }
}

// JW: I guess `union` is used here to save bits
pub(super) union GVNExprPayLoad {
    default: DefaultExprPayLoad,
    // this is basically copy but that just makes the API of ValueList really awkward
    phi: ManuallyDrop<PhiExprPayLoad>,
    call: ManuallyDrop<CallExprPayLoad>,
}

impl GVNExprPayLoad {
    pub(super) fn default(&self) -> DefaultExprPayLoad {
        unsafe { self.default }
    }
    fn phi(&self) -> &PhiExprPayLoad {
        unsafe { &self.phi }
    }
    fn phi_mut(&mut self) -> &mut PhiExprPayLoad {
        unsafe { &mut self.phi }
    }
    fn call(&self) -> &CallExprPayLoad {
        unsafe { &self.call }
    }
    fn call_mut(&mut self) -> &mut CallExprPayLoad {
        unsafe { &mut self.call }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DefaultExprPayLoad {
    // We do not have operation using 3 arguments so this is fine.
    pub(super) val1: Value,
    pub(super) val2: PackedOption<Value>,
}

#[derive(Clone, Debug)]
struct PhiExprPayLoad {
    /// The basic block where the phi instruction resides in, not the blocks of phi edges.
    bb: Block,
    args: ValueList,
}

#[derive(Clone, Debug)]
struct CallExprPayLoad {
    func_ref: FuncRef,
    args: ValueList,
}
