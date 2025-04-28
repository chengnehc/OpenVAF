//! Deal with parameter constraint check and invalid value detection.
//!
//! Refer to:
//! - [LRM 3.4.2] Value range specification
//! - [LRM A.2.5] Declaration ranges

use std::mem;
use stdx::packed_option::ReservedValue;

use hir::{CompilationDB, ConstraintValue, ParamConstraint, Parameter, Type};
use lasso::Rodeo;
use mir::builder::InstBuilder;
use mir::{Block, FuncRef, Function, Opcode, Value, FALSE, GRAVESTONE};
use mir_build::{FunctionBuilder, FunctionBuilderContext};
use syntax::ast::ConstraintKind;

use crate::body::BodyLowerContext;
use crate::ctx::MainLowerContext;
use crate::{CallBackKind, HirInterner, ParamKind, PlaceKind};

#[cfg(test)]
mod tests;

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
    lt: Option<Opcode>, // strict less than
    le: Option<Opcode>, // less than or equal
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
    // TODO(JW): Out-of-range parameter default value should be validated and reported
    // earlier as a compile error. However, OpenVAF currently does this at run-time.
    pub fn insert_param_init(
        &mut self,
        db: &CompilationDB,
        func: &mut Function,
        literals: &mut Rodeo,
        build_min_max: bool, // for parameter extraction backend
        build_stores: bool,  // for simulator backend
        params: &[Parameter],
    ) {
        let mut default_vals = if build_stores { vec![GRAVESTONE; params.len()] } else { vec![] };

        let f_inf = mir::INFINITY;
        let f_neg_inf = func.dfg.fconst(f64::NEG_INFINITY.into());
        let i_inf = func.dfg.iconst(i32::MAX);
        let i_neg_inf = func.dfg.iconst(i32::MIN);

        let mut func_ctxt = FunctionBuilderContext::default();
        let (builder, term) = FunctionBuilder::edit(func, literals, &mut func_ctxt, false);
        let mut ctxt = MainLowerContext::new(db, builder, self);

        for (i, param) in params.iter().copied().enumerate() {
            let mut param_val = ctxt.use_param(ParamKind::Param(param));
            let param_given = ctxt.use_param(ParamKind::ParamGiven { param });

            // create a temporary to hold onto the uses
            let new_val = ctxt.func.make_param(0u32.into());
            ctxt.dfg_mut().replace_uses(param_val, new_val);

            dbg!(new_val, param_val);

            let init = param.init(db);
            let ty = param.ty(db);
            let constraints = param.constraints(db);

            let ops = CmpOps::from_ty(&ty);
            let invalid = ctxt.dec_callback(CallBackKind::ParamInfo(ParamInfoKind::Invalid, param));

            let (then_src, else_src) = ctxt.make_if_stmt(param_given, |ctxt, param_given| {
                if param_given {
                    if build_stores {
                        let exit = ctxt.create_block();
                        let mut ctx = BodyLowerContext { ctxt, body: init.borrow(), path: "" };
                        ctx.check_constraints(param_val, &constraints, &[], ops, invalid, exit);
                        ctxt.switch_to_block(exit);
                    }
                    param_val
                } else {
                    let default_val = ctxt.lower_expr_body(init.borrow(), 0);
                    if build_stores {
                        // JW: Default value range check should be promoted to compile-time,
                        // as default value is written in Verilog-A source code, instead of being
                        // provided by the simulator. In other words, it is statically available.
                        //
                        // As is specified by LRM, "Range checking applies to the value of the parameter
                        // for the instance and not against the default values specified in the device",
                        // saying default value range check is simply not needed, but after all the
                        // default should fall into the range specified by the constraints, shouldn't it?
                        //
                        // let exit = ctxt.create_block();
                        // let mut ctx = BodyLowerContext { ctxt, body: init.borrow(), path: "" };
                        // ctx.check_param(default_val, &constraints, &[], ops, invalid, exit);
                        // ctxt.switch_to_block(exit);
                        //
                        default_vals[i] = ctxt.ins().optbarrier(default_val);
                    }
                    default_val
                }
            });
            ctxt.ins().with_result(new_val).phi(&[then_src, else_src]);

            // Pascal: we purposefully insert these overrides here (new_val -> params, old val -> outputs).
            // This ensures that the MIR generated for other parameters uses the correct value (new_val).
            // Once MIR generation is complete we swap these two again.
            //
            // JW: I suppose this helps to deal with situations where parameters interact with each other,
            // especially when building setup_instance()
            ctxt.intern_param(ParamKind::Param(param), new_val);
            ctxt.intern_output(PlaceKind::Param(param), param_val);
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

                let mut ctx = BodyLowerContext { ctxt: &mut ctxt, body: init.borrow(), path: "" };
                let mut lowered_bounds = None;
                let precomputes: Vec<_> = constraints
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
                                let start = ctx.lower_expr(range.lower_bound);
                                let end = ctx.lower_expr(range.upper_bound);

                                if let Some((min, max)) = lowered_bounds {
                                    let (op, call) = if range.lower_inclusive {
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

                                    let (op, call) = if range.upper_inclusive {
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
                                    if range.lower_inclusive {
                                        ctx.ctxt.ins().call(min_inclusive, &[]);
                                    } else {
                                        ctx.ctxt.ins().call(min_exclusive, &[]);
                                    }

                                    if range.upper_inclusive {
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

                ctxt.intern.outputs.insert(PlaceKind::ParamMin(param), min.into());
                ctxt.intern.outputs.insert(PlaceKind::ParamMax(param), max.into());

                // first from bounds (here we also get min/max from)
                let exit = ctxt.create_block();
                let mut ctx = BodyLowerContext { ctxt: &mut ctxt, body: init.borrow(), path: "" };
                ctx.check_constraints(param_val, &constraints, &precomputes, ops, invalid, exit);
                ctxt.switch_to_block(exit);
            }
        }

        ctxt.ensure_sealed();
        let block = ctxt.current_block();
        ctxt.layout_mut().append_inst_to_block(term, block);

        // The MIR generation is complete and we swap param_val and output_val again. See the comments above.
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
    /// When multiple inclusion ranges are specified, we should use their union, not intersection.
    /// - exit once we find a range which the parameter falls into.
    /// - call Invalid callback only if we have checked *all* the candidate ranges.
    ///
    /// For exclusions, if the parameter value falls out of *any* exclusion range, Invalid callback
    /// should be called.
    fn check_constraints(
        &mut self,
        param_val: Value,
        constraints: &[ParamConstraint],
        precomputes: &[(Value, Value)],
        ops: CmpOps,
        invalid: FuncRef,
        global_exit: Block,
    ) {
        let includes =
            constraints.iter().filter(|it| it.kind == ConstraintKind::Include).enumerate();
        let excludes =
            constraints.iter().filter(|it| it.kind == ConstraintKind::Exclude).enumerate();

        // check all include constraints first
        let mut exit = None;
        for (i, constraint) in includes {
            let exit = exit.get_or_insert_with(|| self.ctxt.create_block());
            self.check_constraints_inner(param_val, (i, constraint), precomputes, ops, *exit);
        }

        if let Some(exit) = exit {
            self.ctxt.ins().call(invalid, &[]);
            self.ctxt.ins().jump(global_exit);
            self.ctxt.switch_to_block(exit);
        }

        // Then, check all exclude constraints
        let mut exit = None;
        for (i, constraint) in excludes {
            let exit = exit.get_or_insert_with(|| self.ctxt.create_block());
            self.check_constraints_inner(param_val, (i, constraint), precomputes, ops, *exit);
        }

        self.ctxt.ins().jump(global_exit);
        if let Some(exit) = exit {
            // for exclusions, entering exit block means that parameter value is invalid
            self.ctxt.switch_to_block(exit);
            self.ctxt.ins().call(invalid, &[]);
            self.ctxt.ins().jump(global_exit);
        }
    }

    fn check_constraints_inner(
        &mut self,
        param_val: Value,
        (i, constraint): (usize, &ParamConstraint),
        precomputes: &[(Value, Value)],
        ops: CmpOps,
        exit: Block,
    ) {
        match constraint.val {
            ConstraintValue::Value(val) => {
                let val = precomputes.get(i).map_or_else(|| self.lower_expr(val), |(val, _)| *val);

                let is_ok = self.ctxt.ins().binary1(ops.eq, val, param_val);
                let next_bb = self.ctxt.create_block();
                self.ctxt.ins().br(is_ok, exit, next_bb);
                self.ctxt.switch_to_block(next_bb);
            }
            ConstraintValue::Range(range) => {
                let (lower, upper) = precomputes.get(i).map_or_else(
                    || (self.lower_expr(range.lower_bound), self.lower_expr(range.upper_bound)),
                    |(lo, up)| (*lo, *up),
                );

                let op = ops.in_bound(range.lower_inclusive);
                let is_lo_ok = self.ctxt.ins().binary1(op, lower, param_val);
                let is_ok = self.ctxt.make_select_expr(is_lo_ok, |cx, is_lo_ok| {
                    if is_lo_ok {
                        let op = ops.in_bound(range.upper_inclusive);
                        cx.ins().binary1(op, param_val, upper)
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
}
