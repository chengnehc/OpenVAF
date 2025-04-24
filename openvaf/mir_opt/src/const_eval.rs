//! Make use of the numeric functions in Rust stdlib for constant evaluation during
//! compile time. On the other hand, run-time evaluations depend on LLVM instrinsics.
//! Although both of them kind of depend on libm under the hood, there are some
//! inconsistent behaviors between them, and some functions have unspecified precision
//! , so be careful.

use std::mem::size_of_val;

use mir::{Const, Function, Opcode, Value, FALSE, F_ONE, F_ZERO, ONE, TRUE, ZERO};

pub fn eval_const_binary(func: &mut Function, op: Opcode, lhs: Const, rhs: Const) -> Value {
    match (lhs, rhs) {
        (Const::Int(lhs), Const::Int(rhs)) => match op {
            Opcode::Iadd => func.dfg.iconst(lhs + rhs),
            Opcode::Isub => func.dfg.iconst(lhs - rhs),
            Opcode::Imul => func.dfg.iconst(lhs * rhs),
            Opcode::Idiv => func.dfg.iconst(lhs / rhs),
            Opcode::Irem => func.dfg.iconst(lhs % rhs),

            Opcode::Ishl => func.dfg.iconst(lhs << rhs),
            Opcode::Ishr => func.dfg.iconst(lhs >> rhs),
            Opcode::Ixor => func.dfg.iconst(lhs ^ rhs),
            Opcode::Iand => func.dfg.iconst(lhs & rhs),
            Opcode::Ior => func.dfg.iconst(lhs | rhs),

            Opcode::Ilt => (lhs < rhs).into(),
            Opcode::Igt => (lhs > rhs).into(),
            Opcode::Ige => (lhs >= rhs).into(),
            Opcode::Ile => (lhs <= rhs).into(),
            Opcode::Ieq => (lhs == rhs).into(),
            Opcode::Ine => (lhs != rhs).into(),

            _ => unreachable!("invalid int operation {op}"),
        },

        (Const::Float(lhs), Const::Float(rhs)) => {
            let lhs: f64 = lhs.into();
            let rhs: f64 = rhs.into();
            match op {
                Opcode::Fadd => func.dfg.f64const(lhs + rhs),
                Opcode::Fsub => func.dfg.f64const(lhs - rhs),
                Opcode::Fmul => func.dfg.f64const(lhs * rhs),
                Opcode::Fdiv => func.dfg.f64const(lhs / rhs),
                Opcode::Frem => func.dfg.f64const(lhs % rhs),

                Opcode::Flt => (lhs < rhs).into(),
                Opcode::Fgt => (lhs > rhs).into(),
                Opcode::Fge => (lhs >= rhs).into(),
                Opcode::Fle => (lhs <= rhs).into(),
                Opcode::Feq => (lhs == rhs).into(),
                Opcode::Fne => (lhs != rhs).into(),

                Opcode::Hypot => func.dfg.f64const(lhs.hypot(rhs)),
                Opcode::Atan2 => func.dfg.f64const(lhs.atan2(rhs)),
                Opcode::Pow => func.dfg.f64const(lhs.powf(rhs)),

                _ => unreachable!("invalid real operation {op}"),
            }
        }
        _ => match op {
            Opcode::Seq | Opcode::Beq => (lhs == rhs).into(),
            Opcode::Sne | Opcode::Bne => (lhs != rhs).into(),

            _ => unreachable!("invalid operation {op} {lhs:?} {rhs:?}"),
        },
    }
}

pub fn eval_const_unary(func: &mut Function, op: Opcode, arg: Const) -> Value {
    match arg {
        mir::Const::Float(arg) => {
            let arg: f64 = arg.into();
            match op {
                Opcode::Sqrt => func.dfg.f64const(arg.sqrt()),
                Opcode::Exp => func.dfg.f64const(arg.exp()),
                Opcode::Ln => func.dfg.f64const(arg.ln()),
                Opcode::Log => func.dfg.f64const(arg.log10()),
                Opcode::Floor => func.dfg.f64const(arg.floor()),
                Opcode::Ceil => func.dfg.f64const(arg.ceil()),
                Opcode::Sin => func.dfg.f64const(arg.sin()),
                Opcode::Cos => func.dfg.f64const(arg.cos()),
                Opcode::Tan => func.dfg.f64const(arg.tan()),
                Opcode::Asin => func.dfg.f64const(arg.asin()),
                Opcode::Acos => func.dfg.f64const(arg.acos()),
                Opcode::Atan => func.dfg.f64const(arg.atan()),
                Opcode::Sinh => func.dfg.f64const(arg.sinh()),
                Opcode::Cosh => func.dfg.f64const(arg.cosh()),
                Opcode::Tanh => func.dfg.f64const(arg.tanh()),
                Opcode::Asinh => func.dfg.f64const(arg.asinh()),
                Opcode::Acosh => func.dfg.f64const(arg.acosh()),
                Opcode::Atanh => func.dfg.f64const(arg.atanh()),
                Opcode::FIcast => func.dfg.iconst(arg.round() as i32),
                Opcode::FBcast => (arg.abs() != 0.0).into(),
                Opcode::Fneg => func.dfg.f64const(-arg),

                _ => unreachable!("invalid real operation {op}"),
            }
        }
        mir::Const::Int(arg) => match op {
            Opcode::Inot => func.dfg.iconst(!arg),
            Opcode::Ineg => func.dfg.iconst(-arg),
            Opcode::IFcast => func.dfg.f64const(arg as f64),
            Opcode::IBcast => (arg != 0).into(),
            Opcode::Clog2 => {
                let res = 8 * size_of_val(&arg) as i32 - arg.leading_zeros() as i32;
                func.dfg.iconst(res)
            }

            _ => unreachable!("invalid int operation {op}"),
        },
        mir::Const::Bool(true) => match op {
            Opcode::Bnot => FALSE,
            Opcode::BIcast => ONE,
            Opcode::BFcast => F_ONE,

            _ => unreachable!(),
        },
        mir::Const::Bool(false) => match op {
            Opcode::Bnot => TRUE,
            Opcode::BIcast => ZERO,
            Opcode::BFcast => F_ZERO,

            _ => unreachable!(),
        },
        mir::Const::Str(_) => unreachable!(),
    }
}
