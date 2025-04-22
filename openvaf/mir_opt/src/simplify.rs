use std::marker::PhantomData;
use std::mem::swap;

use mir::{
    Function, Inst, InstructionData, Opcode, PhiNode, Value, ValueDef, F_N_ONE, F_ONE, F_TEN,
    F_TWO, F_ZERO, N_ONE, ONE, ZERO,
};

use crate::const_eval::{eval_const_binary, eval_const_unary};

pub trait Arithmetic {
    const NEG: Opcode;
    const ADD: Opcode;
    const SUB: Opcode;
    const MUL: Opcode;
    const DIV: Opcode;
    const ZERO: Value;
    const ONE: Value;
    const N_ONE: Value;
    const DIV_EXACT: bool;
    const HAS_SQRT: bool;
}

impl Arithmetic for f64 {
    const NEG: Opcode = Opcode::Fneg;
    const ADD: Opcode = Opcode::Fadd;
    const SUB: Opcode = Opcode::Fsub;
    const MUL: Opcode = Opcode::Fmul;
    const DIV: Opcode = Opcode::Fdiv;
    const ZERO: Value = F_ZERO;
    const ONE: Value = F_ONE;
    const N_ONE: Value = F_N_ONE;
    const DIV_EXACT: bool = true;
    const HAS_SQRT: bool = true;
}

impl Arithmetic for i32 {
    const NEG: Opcode = Opcode::Ineg;
    const ADD: Opcode = Opcode::Iadd;
    const SUB: Opcode = Opcode::Isub;
    const MUL: Opcode = Opcode::Imul;
    const DIV: Opcode = Opcode::Idiv;
    const ZERO: Value = ZERO;
    const ONE: Value = ONE;
    const N_ONE: Value = N_ONE;
    const DIV_EXACT: bool = false;
    const HAS_SQRT: bool = false;
}

pub struct SimplifyCtx<'a, FP: Arithmetic, M: Fn(Value, &Function) -> Value> {
    pub func: &'a mut Function,
    // dtree: &'a DominatorTree,
    pub map_val_: M,
    pub max_recurse: u32,
    __fp_arithmetic: PhantomData<fn(&FP)>,
}

impl<'a, FP: Arithmetic, M: Fn(Value, &Function) -> Value> SimplifyCtx<'a, FP, M> {
    pub fn new(func: &'a mut Function, map_val_: M) -> SimplifyCtx<'a, FP, M> {
        SimplifyCtx { func, map_val_, max_recurse: 3, __fp_arithmetic: PhantomData }
    }

    #[inline]
    fn map_val(&self, val: Value) -> Value {
        (self.map_val_)(val, self.func)
    }

    fn recurse<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        if self.max_recurse == 0 {
            return None;
        }
        self.max_recurse -= 1;
        let res = f(self);
        self.max_recurse += 1;
        res
    }

    pub fn simplify_inst(&mut self, inst: Inst) -> Option<Value> {
        match self.func.dfg.insts[inst].clone() {
            InstructionData::Unary { opcode, arg } => self.simplify_unary_op(opcode, arg),
            InstructionData::Binary { opcode, args } => {
                self.simplify_binop(opcode, args[0], args[1])
            }
            InstructionData::PhiNode(phi) => self.simplify_phi(phi),
            _ => None,
        }
    }

    pub fn simplify_phi(&mut self, phi: PhiNode) -> Option<Value> {
        let mut iter = self.func.dfg.phi_edges(&phi);
        let (_, all_eq_val) = iter.next()?;
        let all_eq_val = self.map_val(all_eq_val);
        // a phi can be simplified when all the values of its edges are the same.
        iter.all(|(_, val)| self.map_val(val) == all_eq_val).then_some(all_eq_val)
    }

    pub fn simplify_unary_op(&mut self, op: Opcode, val: Value) -> Option<Value> {
        // Try constant evaluation first
        if let Some(const_) = self.func.dfg.value_def(val).as_const() {
            if let Some(val) = eval_const_unary(self.func, op, const_) {
                return Some(val);
            }
        }

        // Try re-using the argument of a possible inverse(reciprocal) instruction.
        // We need to evaluate x = f(v). If v = g(u) was evaluated earlier, and (f, g)
        // is a pair of inverse operations, then we could just make use of u directly,
        // since x = f(g(u)) = u.
        let inv = match op {
            // Turn these two into binary op simplification
            Opcode::Fneg => return self.simplify_sub_inst::<FP>(F_ZERO, val),
            Opcode::Ineg => return self.simplify_sub_inst::<i32>(ZERO, val),

            // self-reciprocal
            Opcode::Inot => Opcode::Inot,
            Opcode::Bnot => Opcode::Bnot,

            // inverse cast is lossless
            // X = FI/IB/FBcast(V), V = IF/BI/BFcast(U)
            Opcode::FIcast => Opcode::IFcast,
            Opcode::IBcast => Opcode::BIcast,
            Opcode::FBcast => Opcode::BFcast,

            // inverse cast is lossy, or no inverse operation available
            Opcode::IFcast
            | Opcode::BIcast
            | Opcode::BFcast
            | Opcode::OptBarrier
            | Opcode::Clog2 => return None,

            Opcode::Sqrt => {
                // X = sqrt(V), V = X * X
                if let Some([x, y]) = self.as_binary(val, Opcode::Fmul) {
                    if x == y {
                        return Some(x);
                    }
                }
                // X = sqrt(V), V = pow(X, 2)
                if let Some([x, y]) = self.as_binary(val, Opcode::Pow) {
                    if y == F_TWO {
                        return Some(x);
                    }
                }
                return None;
            }
            Opcode::Exp => Opcode::Ln,
            Opcode::Ln => Opcode::Exp,
            Opcode::Log => {
                // X = log(V), V = pow(10, Y)
                if let Some([x, y]) = self.as_binary(val, Opcode::Pow) {
                    if x == F_TEN {
                        return Some(y);
                    }
                }
                return None;
            }
            // Idempotent operations
            Opcode::Floor | Opcode::Ceil => {
                if matches!(
                    self.as_any_unary(val),
                    Some((Opcode::IFcast | Opcode::BFcast | Opcode::Ceil | Opcode::Floor, _))
                ) {
                    return Some(val);
                } else {
                    return None;
                }
            }
            Opcode::Sin => Opcode::Asin,
            Opcode::Cos => Opcode::Acos,
            Opcode::Tan => Opcode::Atan,
            Opcode::Asin => Opcode::Sin,
            Opcode::Acos => Opcode::Cos,
            Opcode::Atan => Opcode::Tan,
            Opcode::Sinh => Opcode::Asinh,
            Opcode::Cosh => Opcode::Acosh,
            Opcode::Tanh => Opcode::Atanh,
            Opcode::Asinh => Opcode::Sinh,
            Opcode::Acosh => Opcode::Cosh,
            Opcode::Atanh => Opcode::Tanh,
            _ => unreachable!(""),
        };

        if let Some(arg) = self.as_unary(val, inv) {
            return Some(arg);
        }

        None
    }

    pub fn simplify_binop(&mut self, op: Opcode, mut lhs: Value, mut rhs: Value) -> Option<Value> {
        match op {
            Opcode::Iadd => self.simplify_add_inst::<i32>(lhs, rhs),
            Opcode::Isub => self.simplify_sub_inst::<i32>(lhs, rhs),
            Opcode::Imul => self.simplify_mul_inst::<i32>(lhs, rhs),
            Opcode::Idiv => self.simplify_div_inst::<i32>(lhs, rhs),
            Opcode::Fadd => self.simplify_add_inst::<FP>(lhs, rhs),
            Opcode::Fsub => self.simplify_sub_inst::<FP>(lhs, rhs),
            Opcode::Fmul => self.simplify_mul_inst::<FP>(lhs, rhs),
            Opcode::Fdiv => self.simplify_div_inst::<FP>(lhs, rhs),
            Opcode::Pow => self.simplify_pow_inst(lhs, rhs),

            // We only care about instructions that:
            //
            // * have a derivative
            // * are commonly used in compact models
            //
            // Other (more complex) optimizations are better left to LLVM.
            // So we just const eval (if possible)
            Opcode::Irem
            | Opcode::Ilt
            | Opcode::Igt
            | Opcode::Ige
            | Opcode::Ile
            | Opcode::Flt
            | Opcode::Fgt
            | Opcode::Fge
            | Opcode::Fle
            | Opcode::Ieq
            | Opcode::Feq
            | Opcode::Seq
            | Opcode::Beq
            | Opcode::Ine
            | Opcode::Fne
            | Opcode::Sne
            | Opcode::Bne
            | Opcode::Frem
            | Opcode::Ishl
            | Opcode::Ishr
            | Opcode::Ixor
            | Opcode::Iand
            | Opcode::Hypot
            | Opcode::Atan2
            | Opcode::Ior => self.fold_or_commute_consts(op, &mut lhs, &mut rhs),
            _ => unreachable!(),
        }
    }

    /// Try resolving `val` as the result of an existing unary instruction with opcode `op`.
    /// Return the instruction's argument if successful.
    fn as_unary(&self, val: Value, op: Opcode) -> Option<Value> {
        if let ValueDef::Result(inst, _) = self.func.dfg.value_def(val) {
            if let InstructionData::Unary { opcode, arg } = self.func.dfg.insts[inst] {
                if opcode != op {
                    return None;
                }
                let arg = self.map_val(arg);
                return Some(arg);
            }
        }
        None
    }

    /// Try resolving `val` as the result of an existing binary instruction with opcode `op`.
    /// Return the instruction's arguments if successful.
    fn as_binary(&self, val: Value, op: Opcode) -> Option<[Value; 2]> {
        if let ValueDef::Result(inst, _) = self.func.dfg.value_def(val) {
            if let InstructionData::Binary { opcode, mut args } = self.func.dfg.insts[inst] {
                if opcode != op {
                    return None;
                }
                args[0] = self.map_val(args[0]);
                args[1] = self.map_val(args[1]);
                return Some(args);
            }
        }
        None
    }

    /// Try resolving `val` as the result of an existing unary instruction of any kind.
    /// Return the instruction's opcode and argument if successful.
    fn as_any_unary(&self, val: Value) -> Option<(Opcode, Value)> {
        if let ValueDef::Result(inst, _) = self.func.dfg.value_def(val) {
            if let InstructionData::Unary { opcode, arg } = self.func.dfg.insts[inst] {
                return Some((opcode, arg));
            }
        }
        None
    }

    /// Test if `lhs` and `rhs` are opposite numbers, i.e. LHS + RHS = 0
    fn is_opposite<A: Arithmetic>(&self, lhs: Value, rhs: Value) -> bool {
        // check if expression `lhs = -rhs` or `rhs = -lhs` was defined before
        if self.as_unary(lhs, A::NEG) == Some(rhs) || self.as_unary(rhs, A::NEG) == Some(lhs) {
            return true;
        }
        // check if expressions like these were defined before
        // ```
        // lhs = x - y;
        // rhs = y - x;
        // ```
        if let (Some([x1, y1]), Some([y2, x2])) =
            (self.as_binary(lhs, A::SUB), self.as_binary(rhs, A::SUB))
        {
            return x1 == x2 && y1 == y2;
        }
        false
    }

    /// Const-eval a binary instruction and canonicalize the constant to the RHS if
    /// this is a commutative operation.
    fn fold_or_commute_consts(
        &mut self,
        op: Opcode,
        lhs: &mut Value,
        rhs: &mut Value,
    ) -> Option<Value> {
        if let ValueDef::Const(lhs_) = self.func.dfg.value_def(*lhs) {
            if let ValueDef::Const(rhs_) = self.func.dfg.value_def(*rhs) {
                return Some(eval_const_binary(self.func, op, lhs_, rhs_));
            }
            // Canonicalize the constant to the RHS if this is a commutative operation.
            if op.is_commutative() {
                swap(lhs, rhs)
            }
        }
        None
    }

    /// Given operands for an `A::ADD` instruction, see if we can fold the result.
    /// If not, this returns None.
    fn simplify_add_inst<A: Arithmetic>(
        &mut self,
        mut lhs: Value,
        mut rhs: Value,
    ) -> Option<Value> {
        if let Some(val) = self.fold_or_commute_consts(A::ADD, &mut lhs, &mut rhs) {
            return Some(val);
        }

        if rhs == A::ZERO {
            return Some(lhs);
        }

        if self.is_opposite::<A>(lhs, rhs) {
            return Some(A::ZERO);
        }

        // X + (Y - X) -> Y
        if let Some([y, x]) = self.as_binary(rhs, A::SUB) {
            if x == lhs {
                return Some(y);
            }
        }

        // (Y - X) + X -> Y
        if let Some([y, x]) = self.as_binary(lhs, A::SUB) {
            if x == rhs {
                return Some(y);
            }
        }

        // Try some generic simplifications for associative operations.
        self.simplify_assoc_binop(A::ADD, lhs, rhs)
    }

    /// Given operands for an `A::SUB` instruction, see if we can fold the result.
    /// If not, this returns None.
    fn simplify_sub_inst<A: Arithmetic>(
        &mut self,
        mut lhs: Value,
        mut rhs: Value,
    ) -> Option<Value> {
        if let Some(val) = self.fold_or_commute_consts(A::SUB, &mut lhs, &mut rhs) {
            return Some(val);
        }

        if rhs == A::ZERO {
            return Some(lhs);
        }

        if lhs == rhs {
            return Some(A::ZERO);
        }

        self.recurse(|sel| sel.simplify_sub_inst_inner::<A>(lhs, rhs))
    }

    fn simplify_sub_inst_inner<A: Arithmetic>(&mut self, lhs: Value, rhs: Value) -> Option<Value> {
        // (X + Y) - Z -> X + (Y - Z) or Y + (X - Z) if everything simplifies.
        // For example, (X + Y) - Y -> X; (Y + X) - Y -> X
        let z = rhs;
        if let Some([x, y]) = self.as_binary(lhs, A::ADD) {
            // See if "V === Y - Z" simplifies.
            if let Some(v) = self.simplify_sub_inst::<A>(y, z) {
                // It does!  Now see if "X + V" simplifies.
                if let Some(w) = self.simplify_add_inst::<A>(x, v) {
                    return Some(w);
                }
            }

            // See if "V === X - Z" simplifies.
            if let Some(v) = self.simplify_sub_inst::<A>(x, z) {
                // It does!  Now see if "X + V" simplifies.
                if let Some(w) = self.simplify_add_inst::<A>(y, v) {
                    return Some(w);
                }
            }
        }

        // X - (Y + Z) -> (X - Y) - Z or (X - Z) - Y if everything simplifies.
        // For example, X - (X + 1) -> -1
        let x = lhs;
        if let Some([y, z]) = self.as_binary(rhs, A::ADD) {
            // See if "V === X - Y" simplifies.
            if let Some(v) = self.simplify_sub_inst::<A>(x, y) {
                // It does!  Now see if "V - Z" simplifies.
                if let Some(w) = self.simplify_sub_inst::<A>(v, z) {
                    return Some(w);
                }
            }

            // See if "V === X - Z" simplifies.
            if let Some(v) = self.simplify_sub_inst::<A>(x, z) {
                // It does!  Now see if "V - Y" simplifies.
                if let Some(w) = self.simplify_sub_inst::<A>(v, y) {
                    return Some(w);
                }
            }
        }

        // Z - (X - Y) -> (Z - X) + Y if everything simplifies.
        // For example, X - (X - Y) -> Y.
        let z = lhs;
        if let Some([x, y]) = self
            .as_binary(rhs, A::SUB)
            .or_else(|| self.as_unary(rhs, A::NEG).map(|rhs| [A::ZERO, rhs]))
        {
            // See if "V === Z - X" simplifies.
            if let Some(v) = self.simplify_sub_inst::<A>(z, x) {
                // It does!  Now see if "V + Y" simplifies.
                if let Some(w) = self.simplify_add_inst::<A>(v, y) {
                    return Some(w);
                }
            }
        }

        None
    }

    /// Given operands for an `A::MUL` instruction, see if we can fold the result.
    /// If not, this returns None.
    fn simplify_mul_inst<A: Arithmetic>(
        &mut self,
        mut lhs: Value,
        mut rhs: Value,
    ) -> Option<Value> {
        if let Some(val) = self.fold_or_commute_consts(A::MUL, &mut lhs, &mut rhs) {
            return Some(val);
        }

        if rhs == A::ONE {
            return Some(lhs);
        }

        if rhs == A::ZERO {
            return Some(A::ZERO);
        }

        // This only holds when division is exact (only fast math)
        if A::DIV_EXACT {
            // Y * (X / Y) -> X
            if let Some([x, y]) = self.as_binary(rhs, A::DIV) {
                if y == lhs {
                    return Some(x);
                }
            }
            // (X / Y) * Y -> X
            if let Some([x, y]) = self.as_binary(lhs, A::DIV) {
                if y == rhs {
                    return Some(x);
                }
            }
        }

        // sqrt(X) * sqrt(X) -> X
        if A::HAS_SQRT {
            if let Some(x) = self.as_unary(lhs, Opcode::Sqrt) {
                if let Some(y) = self.as_unary(rhs, Opcode::Sqrt) {
                    if x == y {
                        return Some(x);
                    }
                }
            }
        }

        // Try some generic simplifications for associative operations.
        if let Some(v) = self.simplify_assoc_binop(A::MUL, lhs, rhs) {
            return Some(v);
        }

        // Mul distributes over Add. Try some generic simplifications based on this.
        // Recursion is always used, so bail out at once if we already hit the limit.
        self.recurse(|sel| {
            if let Some(val) = sel.expand_distributive_law::<A>(lhs, rhs) {
                return Some(val);
            }

            if let Some(val) = sel.expand_distributive_law::<A>(rhs, lhs) {
                return Some(val);
            }

            None
        })
    }

    /// Try simplifying (a + b) * x -> a*x + b*x
    fn expand_distributive_law<A: Arithmetic>(&mut self, val: Value, mult: Value) -> Option<Value> {
        if let Some([lhs, rhs]) = self.as_binary(val, A::ADD) {
            // simplify a*x
            let lhs_ = self.simplify_mul_inst::<A>(lhs, mult)?;
            // simplify b*x
            let rhs_ = self.simplify_mul_inst::<A>(rhs, mult)?;
            // a*x == a && b*x == b ||  a*x == b && b*x == a  =>  (a + b) * x == a + b
            if (lhs_ == lhs && rhs_ == rhs) || (lhs_ == rhs && rhs_ == lhs) {
                return Some(val);
            }
            self.simplify_mul_inst::<A>(lhs_, rhs_)
        } else {
            None
        }
    }

    /// Given operands for an `A::DIV` instruction, see if we can fold the result.
    /// If not, this returns None.
    fn simplify_div_inst<A: Arithmetic>(
        &mut self,
        mut lhs: Value,
        mut rhs: Value,
    ) -> Option<Value> {
        if let Some(val) = self.fold_or_commute_consts(A::DIV, &mut lhs, &mut rhs) {
            return Some(val);
        }

        if lhs == A::ZERO {
            return Some(A::ZERO);
        }

        if rhs == A::ONE {
            return Some(lhs);
        }

        if self.is_opposite::<A>(lhs, rhs) {
            return Some(A::N_ONE);
        }

        if lhs == rhs {
            return Some(A::ONE);
        }

        // This only holds when division is exact (only fast math)
        if A::DIV_EXACT {
            if let Some([x, y]) = self.as_binary(lhs, A::MUL) {
                // (X * Y) / Y -> X
                if y == rhs {
                    // JW: fixed from 'y == lhs' to 'y == rhs'
                    return Some(x);
                }
                // (X * Y) / X -> Y
                if x == rhs {
                    return Some(y);
                }
            }
        }

        None
    }

    /// Given operands for an `Pow` instruction, see if we can fold the result.
    /// If not, this returns None.
    fn simplify_pow_inst(&mut self, mut lhs: Value, mut rhs: Value) -> Option<Value> {
        // TODO(JW): inspect this further
        // This is placed before const fold to avoid inconsistent behaviour between
        // rust powf and LLVM pow
        if rhs == F_ZERO {
            return Some(F_ONE);
        }

        if let Some(val) = self.fold_or_commute_consts(Opcode::Pow, &mut lhs, &mut rhs) {
            return Some(val);
        }

        if lhs == F_ZERO {
            return Some(F_ZERO);
        }

        if rhs == F_ONE {
            return Some(lhs);
        }

        None
    }

    fn simplify_assoc_binop(&mut self, op: Opcode, lhs: Value, rhs: Value) -> Option<Value> {
        self.recurse(move |sel| sel.simplify_assoc_binop_inner(op, lhs, rhs))
    }

    fn simplify_assoc_binop_inner(&mut self, op: Opcode, lhs: Value, rhs: Value) -> Option<Value> {
        let is_commutative = op.is_commutative();

        // (lhs op rhs)  =>  (A op B) op C
        if let Some([a, b]) = self.as_binary(lhs, op) {
            let c = rhs;

            // Does "B op C" simplify?
            if let Some(val) = self.simplify_binop(op, b, c) {
                // B op C = B  =>  A op B op C  <=>  A op B  <=>  lhs
                if val == b {
                    return Some(lhs);
                }
                if let Some(val) = self.simplify_binop(op, a, val) {
                    return Some(val);
                }
            }

            // if commutative: A op B op C  <=>  C op A op B
            if is_commutative {
                // Does "C op A" simplify?
                if let Some(val) = self.simplify_binop(op, c, a) {
                    // C op A = A  =>  C op A op B  <=>  A op B  <=>  lhs
                    if val == a {
                        return Some(lhs);
                    }
                    if let Some(val) = self.simplify_binop(op, val, b) {
                        return Some(val);
                    }
                }
            }
        }

        // (lhs op rhs)  =>  A op (B op C)
        if let Some([b, c]) = self.as_binary(rhs, op) {
            let a = lhs;

            // Does "A op B" simplify?
            if let Some(val) = self.simplify_binop(op, a, b) {
                // A op B = B  =>  A op B op C  <=>  B op C  <=>  rhs
                if val == b {
                    return Some(rhs);
                }
                if let Some(val) = self.simplify_binop(op, val, c) {
                    return Some(val);
                }
            }

            // if commutative: A op B op C  <=>  B op C op A
            if is_commutative {
                // Does "C op A" simplify?
                if let Some(val) = self.simplify_binop(op, c, a) {
                    // C op A = C  =>  B op C op A  <=>  B op C  <=>  rhs
                    if val == c {
                        return Some(rhs); // JW: fixed from `lhs` to `rhs`
                    }
                    if let Some(val) = self.simplify_binop(op, b, val) {
                        return Some(val);
                    }
                }
            }
        }

        None
    }
}
