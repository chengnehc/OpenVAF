use std::mem;
use stdx::packed_option::ReservedValue;

use hir::{CompilationDB, ConstraintValue, ParamConstraint, Parameter, Type};
use lasso::Rodeo;
use mir::builder::InstBuilder;
use mir::{Block, FuncRef, Function, Opcode, Value, FALSE, GRAVESTONE, INFINITY};
use mir_build::{FunctionBuilder, FunctionBuilderContext};
use syntax::ast::ConstraintKind;

use crate::body::BodyLowerContext;
use crate::ctx::MainLowerContext;
use crate::{CallBackKind, HirInterner, ParamKind, PlaceKind};

#[derive(Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum ParamInfoKind {
    Invalid,
    MinInclusive,
    MaxInclusive,
    MinExclusive,
    MaxExclusive,
}

#[derive(Clone, Copy, Debug)]
struct CmpOps {
    lt: Option<Opcode>,
    le: Option<Opcode>,
    eq: Opcode,
}

impl CmpOps {
    fn from_ty(ty: &Type) -> Self {
        match ty {
            Type::Integer => {
                CmpOps { lt: Some(Opcode::Ilt), le: Some(Opcode::Ile), eq: Opcode::Ieq }
            }
            Type::Real => CmpOps { lt: Some(Opcode::Flt), le: Some(Opcode::Fle), eq: Opcode::Feq },
            Type::String => CmpOps { lt: None, le: None, eq: Opcode::Seq },
            Type::Array { ty, .. } => Self::from_ty(ty),
            Type::EmptyArray => CmpOps { lt: None, le: None, eq: Opcode::Ieq },
            _ => unreachable!(),
        }
    }

    fn in_bound(self, inclusive: bool) -> Opcode {
        if inclusive {
            self.le.unwrap()
        } else {
            self.lt.unwrap()
        }
    }
}

impl HirInterner {
    pub fn insert_param_init(
        &mut self,
        db: &CompilationDB,
        func: &mut Function,
        literals: &mut Rodeo,
        build_min_max: bool,
        build_stores: bool,
        params: &[Parameter],
    ) {
        let mut default_vals = if build_stores { vec![GRAVESTONE; params.len()] } else { vec![] };

        let f_inf = INFINITY;
        let f_neg_inf = func.dfg.fconst(f64::NEG_INFINITY.into());
        let i_inf = func.dfg.iconst(i32::MAX);
        let i_neg_inf = func.dfg.iconst(i32::MIN);

        let mut func_ctxt = FunctionBuilderContext::default();
        let (builder, term) = FunctionBuilder::edit(func, literals, &mut func_ctxt, false);
        let mut ctxt = MainLowerContext::new(db, builder, true, self);

        for (i, param) in params.iter().copied().enumerate() {
            let mut param_val = ctxt.use_param(ParamKind::Param(param));
            let param_given = ctxt.use_param(ParamKind::ParamGiven { param });

            // create a temporary to hold onto the uses
            let new_val = ctxt.func.make_param(0u32.into());
            ctxt.dfg_mut().replace_uses(param_val, new_val);

            let body = param.init(db);
            let ty = param.ty(db);
            let bounds = param.bounds(db);

            let ops = CmpOps::from_ty(&ty);
            let invalid = ctxt.dec_callback(CallBackKind::ParamInfo(ParamInfoKind::Invalid, param));

            let (then_src, else_src) = ctxt.make_if_stmt(param_given, |ctxt, param_given| {
                if param_given {
                    if build_stores {
                        let exit = ctxt.create_block();
                        let mut ctx = BodyLowerContext { ctxt, body: body.borrow(), path: "" };
                        ctx.check_param(
                            param_val,
                            &bounds,
                            &[],
                            ConstraintKind::From,
                            ops,
                            invalid,
                            exit,
                        );
                        ctx.check_param(
                            param_val,
                            &bounds,
                            &[],
                            ConstraintKind::Exclude,
                            ops,
                            invalid,
                            exit,
                        );
                        ctx.ctxt.switch_to_block(exit);
                    }
                    param_val
                } else {
                    let default_val = ctxt.lower_expr_body(body.borrow(), 0);
                    if build_stores {
                        let exit = ctxt.create_block();
                        let mut ctx = BodyLowerContext { ctxt, body: body.borrow(), path: "" };
                        ctx.check_param(
                            default_val,
                            &bounds,
                            &[],
                            ConstraintKind::From,
                            ops,
                            invalid,
                            exit,
                        );
                        ctx.check_param(
                            default_val,
                            &bounds,
                            &[],
                            ConstraintKind::Exclude,
                            ops,
                            invalid,
                            exit,
                        );
                        ctx.ctxt.switch_to_block(exit);
                        default_vals[i] = ctx.ctxt.ins().optbarrier(default_val);
                    }
                    default_val
                }
            });

            // let last_inst = builder.func.layout.last_inst(else_src.0).unwrap();
            ctxt.ins().with_result(new_val).phi(&[then_src, else_src]);

            // we purposefully insert these reversed here (new val into params and old val into
            // outputs). This ensures that the code generated for other parameters uses the
            // correct value. After code generation is complete we swap these two again
            ctxt.def_param(ParamKind::Param(param), new_val);
            ctxt.def_output(PlaceKind::Param(param), param_val);
            param_val = new_val;

            // for verilog-ae
            if build_min_max {
                let invalid =
                    ctxt.dec_callback(CallBackKind::ParamInfo(ParamInfoKind::Invalid, param));
                let min_inclusive =
                    ctxt.dec_callback(CallBackKind::ParamInfo(ParamInfoKind::MinInclusive, param));
                let max_inclusive =
                    ctxt.dec_callback(CallBackKind::ParamInfo(ParamInfoKind::MaxInclusive, param));
                let min_exclusive =
                    ctxt.dec_callback(CallBackKind::ParamInfo(ParamInfoKind::MinExclusive, param));
                let max_exclusive =
                    ctxt.dec_callback(CallBackKind::ParamInfo(ParamInfoKind::MaxExclusive, param));

                let mut ctx = BodyLowerContext { ctxt: &mut ctxt, body: body.borrow(), path: "" };
                let mut lowered_bounds = None;
                let precomputed_vals: Vec<_> = bounds
                    .iter()
                    .filter_map(|bound| {
                        if matches!(bound.kind, ConstraintKind::Exclude) {
                            return None;
                        }
                        let (val0, val1) = match bound.val {
                            ConstraintValue::Value(val) => {
                                let val = ctx.lower_expr(val);

                                if let Some((min, max)) = lowered_bounds {
                                    let is_min = ctx.ctxt.ins().binary1(ops.le.unwrap(), val, min);
                                    let min =
                                        ctx.ctxt.make_select_expr(is_min, |builder, is_min| {
                                            if is_min {
                                                builder.ins().call(min_inclusive, &[]);
                                                val
                                            } else {
                                                min
                                            }
                                        });

                                    let is_max = ctx.ctxt.ins().binary1(ops.le.unwrap(), max, val);
                                    let max =
                                        ctx.ctxt.make_select_expr(is_max, |builder, is_max| {
                                            if is_max {
                                                builder.ins().call(max_inclusive, &[]);
                                                val
                                            } else {
                                                min
                                            }
                                        });

                                    lowered_bounds = Some((min, max));
                                } else if ops.le.is_some() {
                                    lowered_bounds = Some((val, val));
                                    ctx.ctxt.ins().call(min_inclusive, &[]);
                                    ctx.ctxt.ins().call(max_inclusive, &[]);
                                }
                                (val, Value::reserved_value())
                            }
                            ConstraintValue::Range(range) => {
                                let start = ctx.lower_expr(range.start);
                                let end = ctx.lower_expr(range.end);

                                if let Some((min, max)) = lowered_bounds {
                                    let (op, call) = if range.start_inclusive {
                                        (ops.le.unwrap(), min_inclusive)
                                    } else {
                                        (ops.lt.unwrap(), min_exclusive)
                                    };

                                    let is_min = ctx.ctxt.ins().binary1(op, start, min);
                                    let min =
                                        ctx.ctxt.make_select_expr(is_min, |builder, is_min| {
                                            if is_min {
                                                builder.ins().call(call, &[]);
                                                start
                                            } else {
                                                min
                                            }
                                        });

                                    let (op, call) = if range.end_inclusive {
                                        (ops.le.unwrap(), max_inclusive)
                                    } else {
                                        (ops.lt.unwrap(), max_exclusive)
                                    };

                                    let is_max = ctx.ctxt.ins().binary1(op, max, end);
                                    let max =
                                        ctx.ctxt.make_select_expr(is_max, |builder, is_max| {
                                            if is_max {
                                                builder.ins().call(call, &[]);
                                                start
                                            } else {
                                                min
                                            }
                                        });

                                    lowered_bounds = Some((min, max));
                                } else {
                                    if range.start_inclusive {
                                        ctx.ctxt.ins().call(min_inclusive, &[]);
                                    } else {
                                        ctx.ctxt.ins().call(min_exclusive, &[]);
                                    }

                                    if range.end_inclusive {
                                        ctx.ctxt.ins().call(max_inclusive, &[]);
                                    } else {
                                        ctx.ctxt.ins().call(max_exclusive, &[]);
                                    }

                                    lowered_bounds = Some((start, end));
                                }

                                (start, end)
                            }
                        };

                        Some((val0, val1))
                    })
                    .collect();

                let (min, max) = lowered_bounds.unwrap_or_else(|| match ty {
                    Type::Real => (f_neg_inf, f_inf),
                    Type::Integer => (i_neg_inf, i_inf),
                    _ => unreachable!(),
                });

                ctx.ctxt.intern.outputs.insert(PlaceKind::ParamMin(param), min.into());
                ctx.ctxt.intern.outputs.insert(PlaceKind::ParamMax(param), max.into());

                // first from bounds (here we also get min/max from)
                let exit = ctxt.create_block();
                let mut ctx = BodyLowerContext { ctxt: &mut ctxt, body: body.borrow(), path: "" };
                ctx.check_param(
                    param_val,
                    &bounds,
                    &precomputed_vals,
                    ConstraintKind::From,
                    ops,
                    invalid,
                    exit,
                );
                ctx.check_param(
                    param_val,
                    &bounds,
                    &precomputed_vals,
                    ConstraintKind::Exclude,
                    ops,
                    invalid,
                    exit,
                );
                ctx.ctxt.switch_to_block(exit);
            }
        }
        ctxt.ensure_sealed();
        ctxt.func.func.layout.append_inst_to_block(term, ctxt.current_block());

        for (i, param) in params.iter().copied().enumerate() {
            let val = &mut self.params.raw[&ParamKind::Param(param)];
            let output_val = if build_stores { default_vals[i] } else { *val };
            *val =
                mem::replace(&mut self.outputs[&PlaceKind::Param(param)], Some(output_val).into())
                    .unwrap_unchecked();
        }
    }
}

impl BodyLowerContext<'_, '_, '_> {
    /// Perform parameter bounds check
    #[allow(clippy::too_many_arguments)]
    fn check_param(
        &mut self,
        param_val: Value,
        bounds: &[ParamConstraint],
        precomputed_vals: &[(Value, Value)],
        kind: ConstraintKind,
        ops: CmpOps,
        invalid: FuncRef,
        global_exit: Block,
    ) {
        let mut exit = None;

        for (i, bound) in bounds.iter().enumerate() {
            if bound.kind != kind {
                continue;
            }

            let exit = match exit {
                Some(exit) => exit,
                None => {
                    let bb = self.ctxt.create_block();
                    exit = Some(bb);
                    bb
                }
            };

            match bound.val {
                ConstraintValue::Value(val) => {
                    let val = precomputed_vals
                        .get(i)
                        .map_or_else(|| self.lower_expr(val), |(val, _)| *val);

                    let is_ok = self.ctxt.ins().binary1(ops.eq, val, param_val);
                    let next_bb = self.ctxt.create_block();
                    self.ctxt.ins().br(is_ok, exit, next_bb);
                    self.ctxt.switch_to_block(next_bb);
                }
                ConstraintValue::Range(range) => {
                    let (start, end) = precomputed_vals.get(i).map_or_else(
                        || (self.lower_expr(range.start), self.lower_expr(range.end)),
                        |(start, end)| (*start, *end),
                    );

                    let op = ops.in_bound(range.start_inclusive);
                    let is_lo_ok = self.ctxt.ins().binary1(op, start, param_val);
                    let is_ok = self.ctxt.make_select_expr(is_lo_ok, |builder, is_ok| {
                        if is_ok {
                            let op = ops.in_bound(range.end_inclusive);
                            builder.ins().binary1(op, param_val, end)
                        } else {
                            FALSE
                        }
                    });

                    let next_bb = self.ctxt.create_block();
                    self.ctxt.ins().br(is_ok, exit, next_bb);
                    self.ctxt.switch_to_block(next_bb);
                }
            }
        }

        match kind {
            ConstraintKind::From => {
                if let Some(exit) = exit {
                    // error on fallthrough
                    self.ctxt.ins().call(invalid, &[]);
                    self.ctxt.ins().jump(global_exit);
                    self.ctxt.switch_to_block(exit);
                }
            }
            ConstraintKind::Exclude => {
                self.ctxt.ins().jump(global_exit);

                if let Some(exit) = exit {
                    // error on fallthrough
                    self.ctxt.switch_to_block(exit);
                    self.ctxt.ins().call(invalid, &[]);
                    self.ctxt.ins().jump(global_exit);
                }
            }
        }
    }
}
