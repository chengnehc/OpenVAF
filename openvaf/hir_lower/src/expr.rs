use std::iter::zip;

use hir::builtin::{
    FLICKER_NOISE_NAME, NOISE_TABLE_FILE_NAME, NOISE_TABLE_INLINE_NAME, WHITE_NOISE_NAME,
};
use hir::signatures::{
    ABS_INT, ABS_REAL, BOOL_EQ, DDX_POT, IDTMOD_IC, IDTMOD_IC_MODULUS, IDTMOD_IC_MODULUS_OFFSET,
    IDTMOD_IC_MODULUS_OFFSET_NATURE, IDTMOD_IC_MODULUS_OFFSET_TOL, IDTMOD_NO_IC, IDT_IC,
    IDT_IC_ASSERT, IDT_IC_ASSERT_NATURE, IDT_IC_ASSERT_TOL, IDT_NO_IC, INT_EQ, INT_OP,
    LIMIT_BUILTIN_FUNCTION, MAX_INT, MAX_REAL, NATURE_ACCESS_BRANCH, NATURE_ACCESS_NODES,
    NATURE_ACCESS_NODE_GND, NATURE_ACCESS_PORT_FLOW, REAL_EQ, REAL_OP, SIMPARAM_DEFAULT,
    SIMPARAM_NO_DEFAULT, STR_EQ,
};
use hir::{Body, BuiltIn, Expr, ExprId, Literal, Ref, ResolvedFun, Type};
use mir::builder::InstBuilder;
use mir::{Opcode, Value, FALSE, F_ZERO, GRAVESTONE, INFINITY, TRUE, ZERO};
use mir_build::RetBuilder;
use syntax::ast::{BinaryOp, UnaryOp};

use crate::body::BodyLowerContext;
use crate::fmt::DisplayKind;
use crate::{
    CallBackKind, FlowKind, IdtKind, ImplicitEquationKind, NoiseTable, ParamKind, PlaceKind,
};

/// Match a signature against some expression
macro_rules! match_signature {
    ($signature:ident: $($case:ident $(| $extra_case:ident)* => $res:expr),*) => {
        match $signature {
            $($case $(|$extra_case)* => $res,)*
            signature => unreachable!("invalid signature {:?}", signature)
        }
    };
}

impl BodyLowerContext<'_, '_, '_> {
    pub fn lower_expr(&mut self, expr: ExprId) -> Value {
        let old_loc = self.ctxt.get_srcloc();
        self.ctxt.set_srcloc(mir::SourceLoc::new(u32::from(expr) as i32 + 1));

        let mut val = match self.body.get_expr(expr) {
            Expr::Read(reference) => match reference {
                Ref::Variable(var) => self.ctxt.read_var(var),
                Ref::Parameter(param) => self.ctxt.use_param(ParamKind::Param(param)),
                Ref::ParamSysFun(param) => self.ctxt.use_param(ParamKind::ParamSysFun(param)),
                Ref::FunctionArg(fun) => self.ctxt.use_place(PlaceKind::FunctionArg(fun)),
                Ref::FunctionReturn(fun) => self.ctxt.use_place(PlaceKind::FunctionReturn(fun)),
                Ref::NatureAttr(attr) => self.lower_nth_expr_in_body(attr.value(self.ctxt.db), 0),
            },
            Expr::Literal(lit) => match *lit {
                Literal::Int(val) => self.ctxt.iconst(val),
                Literal::Float(val) => self.ctxt.fconst(val),
                Literal::String(ref val) => self.ctxt.sconst(val),
                Literal::Inf => {
                    // fast path for `inf` as it does not need type cast
                    self.ctxt.set_srcloc(old_loc);
                    match self.body.expr_type(expr) {
                        Type::Integer => return self.ctxt.iconst(i32::MAX),
                        Type::Real => return INFINITY,
                        _ => unreachable!(),
                    }
                }
            },
            Expr::UnaryOp { expr: arg, op } => self.lower_unary_op(expr, arg, op),
            Expr::BinaryOp { lhs, rhs, op } => self.lower_bin_op(expr, lhs, rhs, op),
            Expr::Select { cond, then_expr, else_expr } => {
                let cond = self.lower_expr(cond);
                self.lower_select(
                    cond,
                    |mut cx| cx.lower_expr(then_expr),
                    |mut cx| cx.lower_expr(else_expr),
                )
            }
            Expr::Call { args, fun } => match fun {
                ResolvedFun::User { func, limit } => self.lower_user_fun(func, limit, args),
                ResolvedFun::BuiltIn(builtin) => self.lower_builtin(expr, builtin, args),
            },
            Expr::Array(vals) => self.lower_array(expr, vals),
        };
        // apply type cast if necessary
        if let Some((src_ty, dst_ty)) = self.body.need_type_cast(expr) {
            val = self.ctxt.make_type_cast(val, &src_ty, dst_ty)
        };
        self.ctxt.set_srcloc(old_loc);

        val
    }

    fn lower_nth_expr_in_body(&mut self, body: Body, i: usize) -> Value {
        let expr = body.borrow().get_nth_entry_expr(i);
        BodyLowerContext { ctxt: self.ctxt, body: body.borrow(), path: self.path }.lower_expr(expr)
    }

    fn lower_unary_op(&mut self, expr: ExprId, arg: ExprId, op: UnaryOp) -> Value {
        let val = self.lower_expr(arg);
        match op {
            UnaryOp::Identity => val,
            UnaryOp::BitNegate => self.ctxt.ins().inot(val), // JW: ineg is 2's complement, should be inot
            UnaryOp::Not => self.ctxt.ins().bnot(val),
            UnaryOp::Neg => {
                if self.body.as_literal(arg) == Some(&Literal::Inf) {
                    // Special case INFINITY
                    match self.body.expr_type(arg) {
                        Type::Real => self.ctxt.fconst(f64::NEG_INFINITY.into()),
                        Type::Integer => self.ctxt.iconst(i32::MIN),
                        ty => unreachable!("{ty:?}"),
                    }
                } else {
                    match self.body.get_call_signature(expr) {
                        INT_OP => self.ctxt.ins().ineg(val),
                        REAL_OP => self.ctxt.ins().fneg(val),
                        sig => unreachable!("{sig:?}"),
                    }
                }
            }
        }
    }

    fn lower_bin_op(&mut self, expr: ExprId, lhs: ExprId, rhs: ExprId, op: BinaryOp) -> Value {
        let signature = self.body.get_call_signature(expr);
        let opcode = match op {
            // Short-circuit evaluation
            BinaryOp::BooleanOr => {
                // lhs || rhs  <=>  if lhs { true } else { rhs }
                let lhs = self.lower_expr(lhs);
                return self.lower_select(lhs, |_| TRUE, |mut cx| cx.lower_expr(rhs));
            }
            BinaryOp::BooleanAnd => {
                // lhs && rhs  <=>  if lhs { rhs } else { false }
                let lhs = self.lower_expr(lhs);
                return self.lower_select(lhs, |mut cx| cx.lower_expr(rhs), |_| FALSE);
            }

            BinaryOp::EqualityTest => match_signature! {
                signature:
                    BOOL_EQ => Opcode::Beq,
                    INT_EQ  => Opcode::Ieq,
                    REAL_EQ => Opcode::Feq,
                    STR_EQ  => Opcode::Seq
            },
            BinaryOp::NegatedEqualityTest => match_signature! {
                signature:
                    BOOL_EQ => Opcode::Bne,
                    INT_EQ  => Opcode::Ine,
                    REAL_EQ => Opcode::Fne,
                    STR_EQ  => Opcode::Sne
            },
            BinaryOp::GreaterEqualTest => {
                match_signature!(signature: INT_OP => Opcode::Ige, REAL_OP => Opcode::Fge)
            }
            BinaryOp::GreaterTest => {
                match_signature!(signature: INT_OP => Opcode::Igt, REAL_OP => Opcode::Fgt)
            }
            BinaryOp::LesserEqualTest => {
                match_signature!(signature: INT_OP => Opcode::Ile, REAL_OP => Opcode::Fle)
            }
            BinaryOp::LesserTest => {
                match_signature!(signature: INT_OP => Opcode::Ilt, REAL_OP => Opcode::Flt)
            }

            BinaryOp::Addition => {
                match_signature!(signature: INT_OP => Opcode::Iadd, REAL_OP => Opcode::Fadd)
            }
            BinaryOp::Subtraction => {
                match_signature!(signature: INT_OP => Opcode::Isub, REAL_OP => Opcode::Fsub)
            }
            BinaryOp::Multiplication => {
                match_signature!(signature: INT_OP => Opcode::Imul, REAL_OP => Opcode::Fmul)
            }
            BinaryOp::Division => {
                match_signature!(signature: INT_OP => Opcode::Idiv, REAL_OP => Opcode::Fdiv)
            }
            BinaryOp::Remainder => {
                match_signature!(signature: INT_OP => Opcode::Irem, REAL_OP => Opcode::Frem)
            }
            BinaryOp::Power => Opcode::Pow,

            BinaryOp::LeftShift => Opcode::Ishl,
            BinaryOp::RightShift => Opcode::Ishr,
            BinaryOp::BitwiseOr => Opcode::Ior,
            BinaryOp::BitwiseAnd => Opcode::Iand,
            BinaryOp::BitwiseXor => Opcode::Ixor,
            BinaryOp::BitwiseXnor => {
                let lhs = self.lower_expr(lhs);
                let rhs = self.lower_expr(rhs);
                let res = self.ctxt.ins().ixor(lhs, rhs);
                return self.ctxt.ins().inot(res);
            }
        };

        let lhs = self.lower_expr(lhs);
        let rhs = self.lower_expr(rhs);
        self.ctxt.ins().binary1(opcode, lhs, rhs)
    }

    fn lower_user_fun(&mut self, fun: hir::Function, limit: bool, args: &[ExprId]) -> Value {
        if limit {
            if self.ctxt.no_equations {
                return self.lower_expr(args[0]);
            }
            let new_val = self.lower_expr(args[0]);
            let state = self.ctxt.start_limit(new_val);
            let old_val = self.ctxt.use_param(ParamKind::PrevState(state));
            let enable_lim = self.ctxt.use_param(ParamKind::EnableLim);
            let val = self.lower_select(
                enable_lim,
                |mut cx| {
                    cx.ctxt.def_place(PlaceKind::FunctionArg(fun.arg(0, self.ctxt.db)), new_val);
                    cx.ctxt.def_place(PlaceKind::FunctionArg(fun.arg(1, self.ctxt.db)), old_val);
                    cx.lower_user_fun_impl(fun, args, true)
                },
                |_| new_val,
            );
            self.ctxt.finish_limit(state, val)
        } else {
            self.lower_user_fun_impl(fun, args, false)
        }
    }

    fn lower_user_fun_impl(
        &mut self,
        fun: hir::Function,
        args: &[ExprId],
        inside_lim: bool,
    ) -> Value {
        // FIXME proper path for functions
        let mut path = self.path.to_owned();
        path.push_str(&fun.name(self.ctxt.db));

        let mut args = zip(fun.args(self.ctxt.db), args);
        // skip the first two arguments
        if inside_lim {
            args.next();
            args.next();
        }
        for (arg, expr) in args.clone() {
            let init = if arg.is_input(self.ctxt.db) {
                self.lower_expr(*expr)
            } else {
                match &arg.ty(self.ctxt.db) {
                    Type::Real => F_ZERO,
                    Type::Integer => ZERO,
                    ty => unreachable!("invalid function arg type {ty:?}"),
                }
            };
            self.ctxt.def_place(PlaceKind::FunctionArg(arg), init);
        }

        let init = match &fun.return_ty(self.ctxt.db) {
            Type::Real => F_ZERO,
            Type::Integer => ZERO,
            ty => unreachable!("invalid function return type {:?}", ty),
        };
        self.ctxt.def_place(PlaceKind::FunctionReturn(fun), init);

        let body = fun.body(self.ctxt.db);
        BodyLowerContext { body: body.borrow(), path: self.path, ctxt: self.ctxt }
            .lower_entry_stmts();

        // write outputs back to original (including possibly required cast)
        for (arg, &expr) in args {
            if arg.is_output(self.ctxt.db) {
                let mut val = self.ctxt.use_place(PlaceKind::FunctionArg(arg));
                // casting in reverse here since we write back
                if let Some((dst, src)) = self.body.need_type_cast(expr) {
                    val = self.ctxt.make_type_cast(val, src, &dst)
                }
                let dst = self.body.get_expr(expr).as_assignment_lhs();
                self.ctxt.def_place(dst.into(), val);
            }
        }

        self.ctxt.use_place(PlaceKind::FunctionReturn(fun))
    }

    fn lower_builtin(&mut self, expr: ExprId, builtin: BuiltIn, args: &[ExprId]) -> Value {
        let signature = self.body.get_call_signature(expr);
        match builtin {
            // Math functions
            BuiltIn::acos => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().acos(arg0)
            }
            BuiltIn::acosh => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().acosh(arg0)
            }
            BuiltIn::asin => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().asin(arg0)
            }
            BuiltIn::asinh => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().asinh(arg0)
            }
            BuiltIn::atan => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().atan(arg0)
            }
            BuiltIn::atan2 => {
                let arg0 = self.lower_expr(args[0]);
                let arg1 = self.lower_expr(args[1]);
                self.ctxt.ins().atan2(arg0, arg1)
            }
            BuiltIn::atanh => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().atanh(arg0)
            }
            BuiltIn::cos => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().cos(arg0)
            }
            BuiltIn::cosh => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().cosh(arg0)
            }
            BuiltIn::exp => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().exp(arg0)
            }
            BuiltIn::floor => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().floor(arg0)
            }
            BuiltIn::hypot => {
                let arg0 = self.lower_expr(args[0]);
                let arg1 = self.lower_expr(args[1]);
                self.ctxt.ins().hypot(arg0, arg1)
            }
            BuiltIn::ln => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().ln(arg0)
            }
            BuiltIn::sin => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().sin(arg0)
            }
            BuiltIn::sinh => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().sinh(arg0)
            }
            BuiltIn::sqrt => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().sqrt(arg0)
            }
            BuiltIn::tan => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().tan(arg0)
            }
            BuiltIn::tanh => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().tanh(arg0)
            }
            BuiltIn::clog2 => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().clog2(arg0)
            }
            BuiltIn::log10 | BuiltIn::log => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().log(arg0)
            }
            BuiltIn::ceil => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.ins().ceil(arg0)
            }
            BuiltIn::pow => {
                let arg0 = self.lower_expr(args[0]);
                let arg1 = self.lower_expr(args[1]);
                self.ctxt.ins().pow(arg0, arg1)
            }
            BuiltIn::abs => {
                let (negate, comparison, zero) = match_signature!(signature:
                    ABS_REAL => (Opcode::Fneg, Opcode::Flt, F_ZERO),
                    ABS_INT => (Opcode::Ineg, Opcode::Ilt, ZERO)
                );
                // abs(x)  <=>  if (x < 0) then { -x } else { x }
                let arg0 = self.lower_expr(args[0]);
                let cond = self.ctxt.ins().binary1(comparison, arg0, zero);
                self.lower_select(cond, |body| body.ctxt.ins().unary1(negate, arg0), |_| arg0)
            }
            BuiltIn::max => {
                // max(x, y)  <=>  if (x > y) then { x } else { y }
                let comparison = match_signature!(signature: MAX_REAL => InstBuilder::fgt, MAX_INT => InstBuilder::igt);
                let arg0 = self.lower_expr(args[0]);
                let arg1 = self.lower_expr(args[1]);
                let cond = comparison(self.ctxt.ins(), arg0, arg1);
                self.lower_select(cond, |_| arg0, |_| arg1)
            }
            BuiltIn::min => {
                // min(x, y)  <=>  if (x < y) then { x } else { y }
                let comparison = match_signature!(signature: MAX_REAL => InstBuilder::flt, MAX_INT => InstBuilder::ilt);
                let arg0 = self.lower_expr(args[0]);
                let arg1 = self.lower_expr(args[1]);
                let cond = comparison(self.ctxt.ins(), arg0, arg1);
                self.lower_select(cond, |_| arg0, |_| arg1)
            }

            // Signal access functions
            BuiltIn::flow => {
                match_signature! {
                    signature:
                        NATURE_ACCESS_NODES | NATURE_ACCESS_NODE_GND => {
                            let hi = self.body.into_node(args[0]);
                            let lo = args.get(1).map(|&arg| self.body.into_node(arg));
                            self.ctxt.nodes(hi, lo, |hi, lo| ParamKind::Flow(FlowKind::Unnamed{hi, lo}))
                        },
                        NATURE_ACCESS_BRANCH => self.ctxt.use_param(ParamKind::Flow(
                            FlowKind::Branch(self.body.into_branch(args[0]))
                        )),
                        NATURE_ACCESS_PORT_FLOW => self.ctxt.use_param(ParamKind::Flow(
                            FlowKind::Port(self.body.into_port_flow(args[0]))
                        ))
                }
                // AB: Do not divide flow probe.
                //     Flow unknowns correspond to the flow of a single parallel instance.
                //     HIR equation describes a single parallel instance.
                //     Handle $mfactor at a lower level.
                // let mfactor = self.ctx.use_param(ParamKind::ParamSysFun(ParamSysFun::mfactor));
                // return self.ctx.ins().fdiv(res, mfactor);
            }
            BuiltIn::potential => {
                match_signature! {
                    signature:
                        NATURE_ACCESS_NODES | NATURE_ACCESS_NODE_GND => {
                            let hi = self.body.into_node(args[0]);
                            let lo = args.get(1).map(|&arg| self.body.into_node(arg));
                            self.ctxt.nodes(hi, lo, |hi, lo| ParamKind::Potential{hi, lo})
                        },
                        NATURE_ACCESS_BRANCH => {
                            let branch = self.body.into_branch(args[0]).kind(self.ctxt.db);
                            self.ctxt.nodes(branch.unwrap_hi_node(), branch.lo_node(),
                            |hi, lo| ParamKind::Potential{hi, lo})
                        }
                }
            }

            // Analog operators
            BuiltIn::ddt => {
                if self.ctxt.no_equations {
                    return F_ZERO;
                }
                // JW: tolerance is currently not supported
                let arg = self.lower_expr(args[0]);
                self.ctxt.call1(CallBackKind::TimeDerivative, &[arg])
            }
            BuiltIn::ddx => {
                let val = self.lower_expr(args[0]);
                let unknown = self.lower_expr(args[1]);
                let param = self.ctxt.dfg().value_def(unknown).unwrap_param();
                let kind = match signature {
                    DDX_POT => {
                        let node = self.ctxt.get_param_kind(param).unwrap_potential_node();
                        CallBackKind::NodeDerivative(node)
                    }
                    _ => CallBackKind::Derivative(param),
                };
                self.ctxt.call1(kind, &[val])
            }
            BuiltIn::idt | BuiltIn::idtmod if self.ctxt.no_equations => {
                match signature {
                    IDT_NO_IC => F_ZERO,           // fair enough approximation
                    _ => self.lower_expr(args[1]), // get ic
                }
            }
            BuiltIn::idt => {
                let kind = match_signature! {
                    signature:
                        IDT_NO_IC => IdtKind::Basic,
                        IDT_IC => IdtKind::Ic,
                        // we currently do not support tolerance
                        IDT_IC_ASSERT | IDT_IC_ASSERT_TOL | IDT_IC_ASSERT_NATURE => IdtKind::Assert
                };

                self.lower_integral(kind, args)
            }
            BuiltIn::idtmod => {
                let kind = match_signature! {
                    signature:
                        IDTMOD_NO_IC => IdtKind::Basic,
                        IDTMOD_IC => IdtKind::Ic,
                        IDTMOD_IC_MODULUS => IdtKind::Modulus,
                        // we currently do not support tolerance
                        IDTMOD_IC_MODULUS_OFFSET
                        | IDTMOD_IC_MODULUS_OFFSET_TOL
                        | IDTMOD_IC_MODULUS_OFFSET_NATURE => IdtKind::ModulusOffset
                };

                self.lower_integral(kind, args)
            }
            BuiltIn::limexp => {
                let arg0 = self.lower_expr(args[0]);
                let cut_off = self.ctxt.fconst(1e30f64.ln().into());
                let off = self.ctxt.fconst(1e30f64.into());
                let linearize = self.ctxt.ins().fgt(arg0, cut_off);
                self.ctxt.make_select_expr(linearize, |ctxt, linearize| {
                    if linearize {
                        let delta = ctxt.ins().fsub(arg0, cut_off);
                        let lin = ctxt.ins().fmul(off, delta);
                        ctxt.ins().fadd(off, lin)
                    } else {
                        ctxt.ins().exp(arg0)
                    }
                })
            }

            // Analysis dependent functions
            BuiltIn::analysis => {
                let arg = self.lower_expr(args[0]);
                self.ctxt.call1(CallBackKind::Analysis, &[arg])
            }
            BuiltIn::ac_stim
            | BuiltIn::noise_table
            | BuiltIn::noise_table_log
            | BuiltIn::white_noise
            | BuiltIn::flicker_noise
                if self.ctxt.no_equations =>
            {
                F_ZERO
            }
            BuiltIn::white_noise => {
                // we create a dedicated callback for each noise source
                // by giving every source a unique index. Kind of ineffcient
                // but necessary to avoid accidental correlation/opimization
                // (for example white_noise(x) - white_noise(x) is not zero)
                let idx = self.ctxt.num_noise_sources;
                self.ctxt.num_noise_sources += 1;
                let pwr = self.lower_expr(args[0]);
                let name = if signature == WHITE_NOISE_NAME {
                    let name = self.body.as_literal(args[1]).unwrap().unwrap_str();
                    self.ctxt.func.strlit.get_or_intern(name)
                } else {
                    let name = format!("unnamed{idx}");
                    self.ctxt.func.strlit.get_or_intern(name)
                };
                self.ctxt.call1(CallBackKind::WhiteNoise { name, idx }, &[pwr])
            }
            BuiltIn::flicker_noise => {
                // see above
                let idx = self.ctxt.num_noise_sources;
                self.ctxt.num_noise_sources += 1;
                let pwr = self.lower_expr(args[0]);
                let exp = self.lower_expr(args[1]);
                let name = if signature == FLICKER_NOISE_NAME {
                    let name = self.body.as_literal(args[2]).unwrap().unwrap_str();
                    self.ctxt.func.strlit.get_or_intern(name)
                } else {
                    let name = format!("unnamed{idx}");
                    self.ctxt.func.strlit.get_or_intern(name)
                };
                self.ctxt.call1(CallBackKind::FlickerNoise { name, idx }, &[pwr, exp])
            }
            BuiltIn::noise_table | BuiltIn::noise_table_log => {
                // see above
                // JW: currently not supported
                let idx = self.ctxt.num_noise_sources;
                self.ctxt.num_noise_sources += 1;
                let name = if matches!(signature, NOISE_TABLE_INLINE_NAME | NOISE_TABLE_FILE_NAME) {
                    let name = self.body.as_literal(args[1]).unwrap().unwrap_str();
                    self.ctxt.func.strlit.get_or_intern(name)
                } else {
                    let name = format!("unnamed{idx}");
                    self.ctxt.func.strlit.get_or_intern(name)
                };
                let log = builtin == BuiltIn::noise_table_log;
                let noise_table = NoiseTable::new([(0.0, 0.0)], log, name, idx);
                self.ctxt.call1(CallBackKind::NoiseTable(Box::new(noise_table)), &[])
            }

            // Simulation control system tasks
            BuiltIn::write => {
                self.ins_display(DisplayKind::Display, false, args);
                GRAVESTONE
            }
            BuiltIn::display | BuiltIn::strobe | BuiltIn::monitor => {
                self.ins_display(DisplayKind::Display, true, args);
                GRAVESTONE
            }
            BuiltIn::debug => {
                self.ins_display(DisplayKind::Debug, true, args);
                GRAVESTONE
            }
            BuiltIn::warning => {
                self.ins_display(DisplayKind::Warn, true, args);
                GRAVESTONE
            }
            BuiltIn::error => {
                self.ins_display(DisplayKind::Error, true, args);
                GRAVESTONE
            }
            BuiltIn::info => {
                self.ins_display(DisplayKind::Info, true, args);
                GRAVESTONE
            }
            BuiltIn::fatal => {
                self.ins_display(DisplayKind::Fatal, true, args);
                self.ctxt.ins().ret();

                let unreachable_bb = self.ctxt.create_block();
                self.ctxt.switch_to_block(unreachable_bb);
                self.ctxt.seal_block(unreachable_bb);
                GRAVESTONE
            }
            BuiltIn::finish | BuiltIn::stop => GRAVESTONE,

            // Simulator time system functions
            BuiltIn::abstime => self.ctxt.use_param(ParamKind::Abstime),

            // Analog kernel parameter system functions
            BuiltIn::temperature => self.ctxt.use_param(ParamKind::Temperature),
            BuiltIn::vt => {
                // NIST2010 constants
                // TODO: make KB and Q a database input
                const KB: f64 = 1.3806488e-23;
                const Q: f64 = 1.602176565e-19;
                let fac = self.ctxt.fconst((KB / Q).into());
                let temp = match args.first() {
                    Some(temp) => self.lower_expr(*temp),
                    None => self.ctxt.use_param(ParamKind::Temperature),
                };
                self.ctxt.ins().fmul(fac, temp)
            }
            BuiltIn::simparam => {
                let arg0 = self.lower_expr(args[0]);
                match_signature! {signature:
                    SIMPARAM_NO_DEFAULT => self.ctxt.call1(CallBackKind::SimParam, &[arg0]),
                    SIMPARAM_DEFAULT => {
                        let arg1 = self.lower_expr(args[1]);
                        self.ctxt.call1(CallBackKind::SimParamOpt, &[arg0, arg1])
                    }
                }
            }
            BuiltIn::simparam_str => {
                let arg0 = self.lower_expr(args[0]);
                self.ctxt.call1(CallBackKind::SimParamStr, &[arg0])
            }

            // Explicit binding detection system functions
            BuiltIn::param_given => self
                .ctxt
                .use_param(ParamKind::ParamGiven { param: self.body.into_parameter(args[0]) }),
            BuiltIn::port_connected => {
                self.ctxt.use_param(ParamKind::PortConnected { port: self.body.into_node(args[0]) })
            }

            // Analog kernel control system tasks and functions
            BuiltIn::bound_step => {
                let step_size = self.lower_expr(args[0]);
                self.ctxt.def_place(PlaceKind::BoundStep, step_size);
                GRAVESTONE
            }
            BuiltIn::discontinuity => {
                // AB: Negative literals are represented as UnaryOp::Neg(Literal)
                //     We have a function for that now.
                if self.ctxt.inside_lim && Some(-1) == self.body.as_signed_int_literal(&args[0]) {
                    self.ctxt.call(CallBackKind::LimDiscontinuity, &[]);
                } else {
                    // TODO implement support for discontinuity?
                }
                GRAVESTONE
            }
            // Special case for $limit(), a sysfunc that can be used as analog operator.
            BuiltIn::limit if signature == LIMIT_BUILTIN_FUNCTION && !self.ctxt.no_equations => {
                let new_val = self.lower_expr(args[0]);
                let state = self.ctxt.start_limit(new_val);
                let prev_val = self.ctxt.use_param(ParamKind::PrevState(state));
                let name = self.body.as_literal(args[1]).unwrap().unwrap_str();
                let name = self.ctxt.func.strlit.get_or_intern(name);
                let mut call_args = vec![new_val, prev_val];
                call_args.extend(args[2..].iter().map(|arg| self.lower_expr(*arg)));

                let enable_lim = self.ctxt.use_param(ParamKind::EnableLim);
                let res = self.ctxt.make_select_expr(enable_lim, |func, lim| {
                    if lim {
                        func.call1(
                            CallBackKind::BuiltinLimit { name, num_args: args.len() as u32 },
                            &call_args,
                        )
                    } else {
                        new_val
                    }
                });
                self.ctxt.finish_limit(state, res)
            }

            /* TODO: absdelay
            BuiltIn::absdelay => {
                let arg = self.lower_expr(args[0]);
                let mut delay = self.lower_expr(args[1]);
                let (eq1, res) = self.ctx.implicit_equation(ImplicitEquationKind::Absdelay);
                let (eq2, intermediate) = self.ctx.implicit_equation(ImplicitEquationKind::Absdelay);
                if signature == ABSDELAY_MAX {
                    let max_delay = self.lower_expr(args[2]);
                    let use_delay = self.ctx.ins().fle(delay, max_delay);
                    delay = self.lower_select_with(use_delay, |_| delay, |_| max_delay);
                } else {
                    delay = self.ctx.call1(CallBackKind::StoreDelayTime(eq1), &[delay]);
                }

                let mut resist_val = self.ctx.ins().fsub(res, arg);
                resist_val = self.ctx.ins().fdiv(resist_val, delay);
                self.ctx.def_resist_residual(resist_val, eq1);
                self.ctx.def_react_residual(intermediate, eq1);

                let mut resist_val = self.ctx.ins().fsub(res, intermediate);
                resist_val = self.ctx.ins().fdiv(resist_val, delay);
                self.ctx.def_resist_residual(resist_val, eq2);
                let react_val = self.ctx.ins().fdiv(res, F_THREE);
                self.ctx.def_react_residual(react_val, eq2);

                res
            }*/
            BuiltIn::slew | BuiltIn::transition | BuiltIn::limit | BuiltIn::absdelay => {
                self.lower_expr(args[0])
            }

            it => unreachable!("Unknown or unsupported builtin function \"{it:?}\""),
        }
    }

    fn lower_integral(&mut self, kind: IdtKind, args: &[ExprId]) -> Value {
        let (equation, val) = self.ctxt.implicit_equation(ImplicitEquationKind::Idt(kind));

        let mut enable_integral = self.ctxt.use_param(ParamKind::EnableIntegration);
        let residual = if kind.has_ic() {
            if kind.has_assert() {
                enable_integral = self.lower_select(
                    enable_integral,
                    |mut s| {
                        let assert = s.lower_expr(args[2]);
                        s.ctxt.ins().feq(assert, F_ZERO)
                    },
                    |_| FALSE,
                )
            }

            self.lower_multi_select(enable_integral, |mut ctx, branch| {
                if branch {
                    if kind.has_modulus() {
                        let modulus = ctx.lower_expr(args[2]);
                        let (min, max) = if kind.has_offset() {
                            let offset = ctx.lower_expr(args[2]);
                            (offset, ctx.ctxt.ins().fadd(offset, modulus))
                        } else {
                            (F_ZERO, modulus)
                        };
                        let too_large = ctx.ctxt.ins().fgt(val, max);
                        ctx.lower_multi_select(too_large, |mut ctx, too_large| {
                            if too_large {
                                [ctx.ctxt.ins().fsub(val, min), F_ZERO]
                            } else {
                                let too_small = ctx.ctxt.ins().flt(val, min);
                                ctx.lower_multi_select(too_small, |mut ctx, too_small| {
                                    if too_small {
                                        [ctx.ctxt.ins().fsub(val, min), F_ZERO]
                                    } else {
                                        let arg = ctx.lower_expr(args[0]);
                                        [ctx.ctxt.ins().fneg(arg), val]
                                    }
                                })
                            }
                        })
                    } else {
                        let arg = ctx.lower_expr(args[0]);
                        [ctx.ctxt.ins().fneg(arg), val]
                    }
                } else {
                    let ic = ctx.lower_expr(args[1]);
                    [ctx.ctxt.ins().fsub(val, ic), F_ZERO]
                }
            })
        } else {
            let arg = self.lower_expr(args[0]);
            [self.ctxt.ins().fneg(arg), val]
        };

        self.ctxt.def_resist_residual(residual[0], equation);
        self.ctxt.def_react_residual(residual[1], equation);

        val
    }

    fn lower_array(&mut self, _expr: ExprId, _args: &[ExprId]) -> Value {
        todo!("arrays")
    }
}
