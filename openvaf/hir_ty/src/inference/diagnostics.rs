use std::iter::zip;
use stdx::pretty;

use basedb::diagnostics::{
    to_unified_span_list, to_unified_spans, Diagnostic, Label, LabelStyle, Report,
};
use basedb::lints::builtin::non_standard_code;
use basedb::lints::{Lint, LintSrc};
use basedb::{BaseDB, FileId};
use hir_def::body::BodySourceMap;
use hir_def::Lookup;
use syntax::ast::{self, AssignOp};
use syntax::sourcemap::FileSpan;
use syntax::TextSize;

use crate::db::HirTyDB;
use crate::inference::InferDiagnostic;

pub struct InferDiagnosticWrapped<'a> {
    pub db: &'a dyn HirTyDB,
    pub body_src_map: &'a BodySourceMap,
    pub diag: &'a InferDiagnostic,
}

impl Diagnostic for InferDiagnosticWrapped<'_> {
    fn lint(&self, _root_file: FileId, _db: &dyn BaseDB) -> Option<(Lint, LintSrc)> {
        if let InferDiagnostic::NonStandardUnknown { stmt, .. } = *self.diag {
            Some((non_standard_code, self.body_src_map.lint_src(stmt, non_standard_code)))
        } else {
            None
        }
    }

    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let src_map = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        match *self.diag {
            InferDiagnostic::PathResolveError { ref err, expr } => {
                let src = parse.to_file_span(
                    self.body_src_map.expr_map_back[expr].as_ref().unwrap().text_range(),
                    &src_map,
                );

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: src.file,
                        range: src.range.into(),
                        message: err.message(),
                    }])
                    .with_message(err.to_string())
            }

            InferDiagnostic::InvalidAssignDst { expr: e, op_kind, maybe_different_op } => {
                let src = parse.to_file_span(
                    self.body_src_map.expr_map_back[e].as_ref().unwrap().text_range(),
                    &src_map,
                );
                let res = Report::error().with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id: src.file,
                    range: src.range.into(),
                    message: "invalid destination".to_owned(),
                }]);

                let res = match op_kind {
                    AssignOp::Contribute => res
                        .with_message("invalid destination for branch contribution")
                        .with_notes(vec![
                            "help: expected nature access such as V(foo) or I(foo)".to_owned()
                        ]),
                    AssignOp::Assign => res
                        .with_message("invalid destination for assignment")
                        .with_notes(vec!["help: expected a variable".to_owned()]),
                };

                match maybe_different_op {
                    Some(ast::AssignOp::Contribute) => res.with_notes(vec![
                        "help: found a branch access\nperhaps you mean to contribute (<+)"
                            .to_owned(),
                    ]),
                    Some(ast::AssignOp::Assign) => res.with_notes(vec![
                        "help: found a variable\nperhaps you meant to assign (=) a value"
                            .to_owned(),
                    ]),
                    None => res,
                }
            }

            InferDiagnostic::ArgCntMismatch { expected, found, expr, exact } => {
                let src = parse.to_file_span(
                    self.body_src_map.expr_map_back[expr].as_ref().unwrap().text_range(),
                    &src_map,
                );

                let message = match (expected < found, exact) {
                    (_, true) => format!("expected {} arguments", expected),
                    (true, false) => format!("expected at most {} arguments", expected),
                    (false, false) => format!("expected at least {} arguments", expected),
                };

                Report::error()
                    .with_message(format!(
                        "invalid argument count: {} but found {}",
                        &message, found
                    ))
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: src.file,
                        range: src.range.into(),
                        message,
                    }])
            }

            InferDiagnostic::TypeMismatch(ref err) => {
                let src = parse.to_file_span(
                    self.body_src_map.expr_map_back[err.expr].as_ref().unwrap().text_range(),
                    &src_map,
                );

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: src.file,
                        range: src.range.into(),
                        message: err.to_string(),
                    }])
                    .with_message(format!("type mismatch: {} but found {}", &err, err.found_ty))
            }

            InferDiagnostic::SignatureMismatch(ref err) => {
                let mut res = if let [ref ty_err] = *err.type_mismatches {
                    let FileSpan { file, range } = parse.to_file_span(
                        self.body_src_map.expr_map_back[ty_err.expr].as_ref().unwrap().text_range(),
                        &src_map,
                    );

                    Report::error()
                        .with_labels(vec![])
                        .with_message(format!(
                            "type mismatch: {} but found {}",
                            &ty_err, ty_err.found_ty
                        ))
                        .with_labels(vec![Label {
                            style: LabelStyle::Primary,
                            file_id: file,
                            range: range.into(),
                            message: ty_err.to_string(),
                        }])
                } else {
                    let mut spans: Vec<_> = err
                        .type_mismatches
                        .iter()
                        .map(|it| {
                            parse.to_ctx_span(
                                self.body_src_map.expr_map_back[it.expr]
                                    .as_ref()
                                    .unwrap()
                                    .text_range(),
                                &src_map,
                            )
                        })
                        .collect();

                    let (file, ranges) = to_unified_span_list(&src_map, &mut spans);
                    let labels = zip(ranges, &*err.type_mismatches)
                        .map(|(range, ty_err)| Label {
                            style: LabelStyle::Primary,
                            file_id: file,
                            range: range.into(),
                            message: ty_err.to_string(),
                        })
                        .collect();

                    let mut notes = vec![format!(
                        "help: found ({})",
                        pretty::List::new(&*err.found).with_final_separator(", ")
                    )];
                    notes.extend(err.signatures.iter().map(|sig| format!("expected {sig}")));

                    Report::error()
                        .with_message("type mismatch: invalid function arguments".to_owned())
                        .with_labels(labels)
                        .with_notes(notes)
                };

                if let Some(src) = err.src {
                    let fun = src.lookup(self.db.upcast());
                    let name = fun.item_tree(self.db.upcast())[fun.id].name.clone();
                    let range = fun.ast_ptr(self.db.upcast()).text_range();
                    let span = parse.to_file_span(range, &src_map);
                    res.labels.push(Label {
                        style: LabelStyle::Secondary,
                        file_id: span.file,
                        range: span.range.into(),
                        message: format!("info: '{name}' was declared here"),
                    })
                }

                res
            }

            InferDiagnostic::ArrayTypeMismatch {
                ref expected,
                ref found_ty,
                found_expr,
                expected_expr,
            } => {
                let found_range =
                    self.body_src_map.expr_map_back[found_expr].as_ref().unwrap().text_range();
                let expected_range =
                    self.body_src_map.expr_map_back[expected_expr].as_ref().unwrap().text_range();

                let expected_span = parse.to_ctx_span(expected_range, &src_map);
                let found_span = parse.to_ctx_span(found_range, &src_map);

                let (file, [found_range, expected_range]) =
                    to_unified_spans(&src_map, [found_span, expected_span]);

                Report::error()
                    .with_labels(vec![
                        Label {
                            style: LabelStyle::Primary,
                            file_id: file,
                            range: found_range.into(),
                            message: format!("expected {}", expected),
                        },
                        Label {
                            style: LabelStyle::Secondary,
                            file_id: file,
                            range: expected_range.into(),
                            message: format!("expected because this is {}", found_ty),
                        },
                    ])
                    .with_message(format!("type mismatch: {} but found {}", expected, found_ty))
                    .with_notes(vec!["help: all array elements must have the same type".to_owned()])
            }

            InferDiagnostic::InvalidUnknown { expr } => {
                let src = parse.to_file_span(
                    self.body_src_map.expr_map_back[expr].as_ref().unwrap().text_range(),
                    &src_map,
                );

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: src.file,
                        range: src.range.into(),
                        message: "invalid ddx unknown".to_owned(),
                    }])
                    .with_message("invalid unknown was supplied to the ddx operator")
                    .with_notes(vec![
                        "help: expected one of the following\nbranch current access: I(branch), I(a,b)\nnode voltage: V(x)\nexplicit voltage: V(x,y)\ntemperature: $temperature".to_owned(),
                    ])
            }

            InferDiagnostic::NonStandardUnknown { expr, .. } => {
                let src = parse.to_file_span(
                    self.body_src_map.expr_map_back[expr].as_ref().unwrap().text_range(),
                    &src_map,
                );

                Report::warning()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: src.file,
                        range: src.range.into(),
                        message: "unknown is not standard compliant".to_owned(),
                    }])
                    .with_message("unknown supplied to the ddx operator is not standard compliant")
                    .with_notes(vec![
                        "note: this functionality is fully supported by openvaf\nbut other Verilog-A compilers might not support it".to_owned(),
                        "help: expected one of the following\nbranch current access: I(branch), I(a,b)\nnode voltage: V(x)".to_owned(),
                    ])
            }

            InferDiagnostic::ExpectedProbe { expr } => {
                let src = parse.to_file_span(
                    self.body_src_map.expr_map_back[expr].as_ref().unwrap().text_range(),
                    &src_map,
                );

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: src.file,
                        range: src.range.into(),
                        message: "expected a branch probe".to_owned(),
                    }])
                    .with_message("'$limit' expected a branch probe as the first argument")
                    .with_notes(vec![
                        "help: expected nature access such as V(foo) or I(foo)".to_owned()
                    ])
            }

            InferDiagnostic::InvalidLimitFunction {
                expr,
                func,
                invalid_arg0,
                invalid_arg1,
                invalid_ret,
                ref output_args,
            } => {
                let src = parse.to_file_span(
                    self.body_src_map.expr_map_back[expr].as_ref().unwrap().text_range(),
                    &src_map,
                );

                let func = func.lookup(self.db.upcast());
                let name = func.name(self.db.upcast());
                let tree = func.item_tree(self.db.upcast());
                let mut labels = Vec::new();
                let id_map = self.db.ast_id_map(func.scope.root_file);

                let decl = tree[func.id].ast_id;
                let decl = id_map.get_erased(decl.into()).text_range();
                let decl = parse.to_file_span(decl, &src_map);
                labels.push(Label {
                    style: LabelStyle::Primary,
                    file_id: decl.file,
                    range: decl.range.into(),
                    message: "invalid $limit function".to_owned(),
                });

                labels.push(Label {
                    style: LabelStyle::Secondary,
                    file_id: src.file,
                    range: src.range.into(),
                    message: format!("info: {name} is used in $limit here"),
                });

                if invalid_arg0 {
                    let decl = tree[tree[func.id].args.raw[0].var_binds[0]].ast_id;
                    let decl = id_map.get_erased(decl.into()).text_range();
                    let decl = parse.to_file_span(decl, &src_map);
                    labels.push(Label {
                        style: LabelStyle::Secondary,
                        file_id: decl.file,
                        range: decl.range.into(),
                        message: "help: first argument must have 'real' type".to_owned(),
                    })
                }

                if invalid_arg1 {
                    let decl = tree[tree[func.id].args.raw[1].var_binds[0]].ast_id;
                    let decl = id_map.get_erased(decl.into()).text_range();
                    let decl = parse.to_file_span(decl, &src_map);
                    labels.push(Label {
                        style: LabelStyle::Secondary,
                        file_id: decl.file,
                        range: decl.range.into(),
                        message: "help: second argument must have 'real' type".to_owned(),
                    })
                }

                for arg in output_args {
                    let decl = tree[func.id].args[*arg].ast_id;
                    let decl = id_map.get_erased(decl.into()).text_range();
                    let decl = parse.to_file_span(decl, &src_map);
                    labels.push(Label {
                        style: LabelStyle::Secondary,
                        file_id: decl.file,
                        range: decl.range.into(),
                        message: "help: argument must have direction 'input'".to_owned(),
                    })
                }

                let mut notes = Vec::new();

                if invalid_ret {
                    notes.push("help: return type must be real".to_owned());
                }

                Report::error()
                    .with_labels(labels)
                    .with_message(format!("{name} is not a valid function for use with $limit"))
                    .with_notes(notes)
            }

            InferDiagnostic::DisplayTypeMismatch { ref err, fmt_lit, lit_range, .. } => {
                let fmt_lit =
                    self.body_src_map.expr_map_back[fmt_lit].as_ref().unwrap().text_range();
                let lit_src = parse
                    .to_file_span(lit_range + fmt_lit.start() + TextSize::from(1u32), &src_map);

                let val_src = parse.to_file_span(
                    self.body_src_map.expr_map_back[err.expr].as_ref().unwrap().text_range(),
                    &src_map,
                );

                Report::error()
                    .with_labels(vec![
                        Label {
                            style: LabelStyle::Primary,
                            file_id: val_src.file,
                            range: val_src.range.into(),
                            message: err.to_string(),
                        },
                        Label {
                            style: LabelStyle::Secondary,
                            file_id: lit_src.file,
                            range: lit_src.range.into(),
                            message: "help: expected because of this fmt specifier".to_owned(),
                        },
                    ])
                    .with_message(format!("type mismatch: {} but found {}", &err, err.found_ty))
            }

            InferDiagnostic::MissingFmtArg { fmt_lit, lit_range } => {
                let fmt_lit =
                    self.body_src_map.expr_map_back[fmt_lit].as_ref().unwrap().text_range();
                let lit_src = parse
                    .to_file_span(lit_range + fmt_lit.start() + TextSize::from(1u32), &src_map);

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: lit_src.file,
                        range: lit_src.range.into(),
                        message: "value for this fmt specifier is missing".to_owned(),
                    }])
                    .with_message("$display system task is missing an argument")
            }

            InferDiagnostic::InvalidFmtSpecifierChar {
                fmt_lit,
                lit_range,
                err_char,
                candidates,
            } => {
                let fmt_lit =
                    self.body_src_map.expr_map_back[fmt_lit].as_ref().unwrap().text_range();
                let lit_src = parse
                    .to_file_span(lit_range + fmt_lit.start() + TextSize::from(1u32), &src_map);

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: lit_src.file,
                        range: lit_src.range.into(),
                        message: "unexpected character in fmt specifier".to_owned(),
                    }])
                    .with_message(format!(
                        "failed to parse format specifier; unexpected character {err_char}",
                    ))
                    .with_notes(vec![format!(
                        "help: expected {}",
                        pretty::List::new(candidates)
                            .surround("'")
                            .with_first_break_after(15)
                            .with_break_after(18)
                    )])
            }

            InferDiagnostic::InvalidFmtSpecifierEnd { fmt_lit, lit_range } => {
                let fmt_lit =
                    self.body_src_map.expr_map_back[fmt_lit].as_ref().unwrap().text_range();
                let lit_src = parse
                    .to_file_span(lit_range + fmt_lit.start() + TextSize::from(1u32), &src_map);

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: lit_src.file,
                        range: lit_src.range.into(),
                        message: "unexpected end of fmt specifier".to_owned(),
                    }])
                    .with_message(
                        "failed to parse format specifier; unexpected end of literal".to_owned(),
                    )
            }
        }
    }
}
