use super::*;

use basedb::lints::builtin::{const_simparam, trivial_probe, variant_const_simparam};
use hir_def::{ItemLoc, NodeTypeDecl};

impl BodyDiagnosticWrapped<'_> {
    #[inline]
    fn expr_span(&self, expr: ExprId, src_map: &SourceMap, parse: &Parse<SourceFile>) -> FileSpan {
        let range = self.body_src_map[expr].as_ref().unwrap().text_range();
        parse.to_file_span(range, src_map)
    }

    fn lookup<I, T>(&self, id: I) -> (Name, TextRange)
    where
        I: Lookup<Data = ItemLoc<T>>,
        T: ItemTreeNode,
    {
        let db = self.db.upcast();
        let loc = id.lookup(db);
        let name = loc.name(db);
        let range = loc.ast_ptr(db).text_range();

        (name, range)
    }
}

impl Diagnostic for BodyDiagnosticWrapped<'_> {
    fn lint(&self, root_file: FileId, db: &dyn BaseDB) -> Option<(Lint, LintSrc)> {
        match *self.diag {
            BodyDiagnostic::ConstSimparam { known: false, stmt, .. } => {
                let src1 = self.body_src_map.lint_src(stmt, const_simparam);
                let (lvl1, _) = src1.lvl(const_simparam, root_file, db);
                let src2 = self.body_src_map.lint_src(stmt, variant_const_simparam);
                let (lvl2, _) = src2.lvl(variant_const_simparam, root_file, db);

                let res = if lvl2 > lvl1 {
                    (variant_const_simparam, src2)
                } else {
                    (const_simparam, src1)
                };
                Some(res)
            }
            BodyDiagnostic::ConstSimparam { known: true, stmt, .. } => {
                let src = self.body_src_map.lint_src(stmt, const_simparam);
                Some((const_simparam, src))
            }
            BodyDiagnostic::TrivialBranchAccess { stmt, .. } => {
                let src = self.body_src_map.lint_src(stmt, trivial_probe);
                Some((trivial_probe, src))
            }
            _ => None,
        }
    }

    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let src_map = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        match *self.diag {
            BodyDiagnostic::ExpectedPort { expr, node } => {
                let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);

                let ast_id_map = db.ast_id_map(root_file);
                let db = self.db.upcast();
                let node = node.lookup(db);
                let module = node.module.lookup(db);
                let tree = module.item_tree(db);
                let node = &tree[module.id].nodes[node.id];

                let mut labels = vec![Label {
                    style: LabelStyle::Primary,
                    file_id: file,
                    range: range.into(),
                    message: "expected port".to_owned(),
                }];

                labels.extend(node.decls.iter().map(|decl| {
                    let NodeTypeDecl::Net(net) = *decl else { unreachable!() };
                    let range = ast_id_map.get(tree[net].ast_id).text_range();
                    let FileSpan { range, file } = parse.to_file_span(range, &src_map);

                    Label {
                        style: LabelStyle::Secondary,
                        file_id: file,
                        range: range.into(),
                        message: format!("info: '{}' was declared here", node.name),
                    }
                }));

                Report::error()
                    .with_message(format!(
                        "expected a port reference but no direction was declared for net '{}'",
                        node.name
                    ))
                    .with_labels(labels)
                    .with_notes(vec![
                        "help: prefix one of the declarations with inout, input or output"
                            .to_owned(),
                    ])
            }

            /* Branch access */
            BodyDiagnostic::PotentialOfPortFlow { expr, branch } => {
                let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);

                let mut labels = vec![Label {
                    style: LabelStyle::Primary,
                    file_id: file,
                    range: range.into(),
                    message: "invalid potential access".to_owned(),
                }];

                if let Some(branch) = branch {
                    let (name, range) = self.lookup(branch);
                    let FileSpan { range, file } = parse.to_file_span(range, &src_map);
                    labels.push(Label {
                        style: LabelStyle::Secondary,
                        file_id: file,
                        range: range.into(),
                        message: format!("info: '{name}' was declared here"),
                    });
                }

                Report::error()
                    .with_message("access of port-branch potential")
                    .with_labels(labels)
                    .with_notes(vec![
                        "help: only the flow of port branches like <foo> can be accessed"
                            .to_owned(),
                    ])
            }
            BodyDiagnostic::TrivialBranchAccess { branch, expr, .. } => {
                let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);
                let db = self.db.upcast();
                let branch_name = match branch {
                    BranchWrite::Named(branch) => branch.lookup(db).name(db).to_string(),
                    BranchWrite::Unnamed { hi, lo } => {
                        if let Some(lo) = lo {
                            format!("({}, {})", db.node_data(hi).name, db.node_data(lo).name)
                        } else {
                            format!("({})", db.node_data(hi).name)
                        }
                    }
                };
                let branch_probe = match branch {
                    BranchWrite::Named(_) => &branch_name,
                    BranchWrite::Unnamed { .. } => &branch_name[1..branch_name.len() - 1],
                };
                let res = Report::error()
                    .with_message("Current probe always returns zero".to_owned())
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: file,
                        range: range.into(),
                        message: "always returns zero".to_owned(),
                    }]);

                res.with_notes(vec![
                    format!("help: there are no contributions to branch {branch_name}"),
                    format!("info: branches are open circuted by default: I({branch_probe}) <+ 0"),
                ])
            }
            BodyDiagnostic::IncompatibleUnnamedBranch { expr, node1, node2 } => {
                let name1 = &self.db.node_data(node1).name;
                let name2 = &self.db.node_data(node2).name;
                let ast_id_map = db.ast_id_map(root_file);

                IncompatibleBranchDiagnostic {
                    branch_span: self.expr_span(expr, &src_map, &parse),
                    branch_name: format!("({name1}, {name2})"),
                    node1,
                    node2,
                }
                .into_report(self.db, &parse, &ast_id_map, &src_map)
            }
            BodyDiagnostic::IncompatibleNatureAccess {
                ref candidates,
                access_nature,
                access_expr,
                ref branch,
            } => {
                let FileSpan { range, file } = self.expr_span(access_expr, &src_map, &parse);
                let access_nature = access_nature.map(|nature| self.db.nature_data(nature));

                let message = if let Some(access_nature) = access_nature {
                    format!("'{}' is not a valid nature for this branch", access_nature.name)
                } else {
                    "illegal access".to_owned()
                };
                let labels = vec![Label {
                    style: LabelStyle::Primary,
                    file_id: file,
                    range: range.into(),
                    message,
                }];
                let msg = format!("illegal access of branch '{branch}'");
                let help_msg = match candidates {
                    [None, None] => {
                        "help: this branch has a natureless discipline and can't be accessed"
                            .to_owned()
                    }
                    [None, Some((flow, flow_access))] => {
                        format!("help: use '{}' to access '{}' (flow)", flow_access, flow)
                    }
                    [Some((pot, pot_access)), None] => {
                        format!("help: use '{}' to access '{}' (potential)", pot_access, pot)
                    }
                    [Some((pot, pot_access)), Some((flow, flow_access))] => {
                        format!(
                            "help: use '{}' or '{}' to access '{}' (potential) or '{}' (flow)",
                            pot_access, flow_access, pot, flow
                        )
                    }
                };

                Report::error().with_labels(labels).with_message(msg).with_notes(vec![help_msg])
            }
            // this is for potential() or flow() access functions
            BodyDiagnostic::IllegalNatureAccess { is_pot, expr } => {
                let name = if is_pot { "potential" } else { "flow" };
                let span = self.expr_span(expr, &src_map, &parse);

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: span.file,
                        range: span.range.into(),
                        message: format!("access of branch without {name}"),
                    }])
                    .with_message(format!("'{name}' access of branch without {name}"))
                    .with_notes(vec![format!(
                        "help: this branch belongs to a discipline without '{name}' attribute"
                    )])
            }

            /* Context violation */
            BodyDiagnostic::IllegalContribute { stmt, ctxt } => {
                let range = self.body_src_map[stmt].as_ref().unwrap().text_range();
                let FileSpan { range, file } = parse.to_file_span(range, &src_map);

                Report::error()
                    .with_message(format!("branch contributions are not allowed in {ctxt}"))
                    .with_labels(vec![Label {
                        style: LabelStyle::Secondary,
                        file_id: file,
                        range: range.into(),
                        message: "not allowed here".to_owned(),
                    }])
                    .with_notes(vec![
                        "help: branch contributions are only allowed in module-level analog blocks"
                            .to_owned(),
                    ])
            }
            BodyDiagnostic::IllegalCtxtAccess { ref kind, ctxt, expr } => {
                let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);

                let mut res = Report::error().with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id: file,
                    range: range.into(),
                    message: "not allowed here".to_owned(),
                }]);

                match kind {
                    IllegalCtxtAccessKind::NatureAccess => res
                        .with_message(format!("nature access is not allowed in {ctxt}"))
                        .with_notes(vec![
                            "help: nature access is only allowed in module-level analog blocks"
                                .to_owned(),
                        ]),
                    IllegalCtxtAccessKind::AnalogOperator {
                        name,
                        is_standard: _, // TODO add a note?
                        non_const_dominator,
                    } => {
                        let notes = if ctxt == BodyContext::Conditional {
                            vec![
                                "help: analog operators shall not be used inside conditional (if, case, or ?:) \n\
                                statements unless the conditional expression controlling the statement consists\n\
                                of terms which can not change their value during simulation".to_owned(),
                            ]
                        } else {
                            vec!["help: analog operators are only allowed in module-level analog blocks"
                                .to_owned()]
                        };
                        res.labels.extend(non_const_dominator.iter().map(|&expr| {
                            let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);
                            Label {
                                style: LabelStyle::Secondary,
                                file_id: file,
                                range: range.into(),
                                message: "help: this condition is not a constant".to_owned(),
                            }
                        }));
                        res.with_message(format!(
                            "analog operator '{name}' is not allowed in {ctxt}",
                        ))
                        .with_notes(notes)
                    }
                    IllegalCtxtAccessKind::AnalysisFun { name } => res.with_message(format!(
                        "analysis function '{name}' is not allowed in constants",
                    )),
                    IllegalCtxtAccessKind::Var(var) => {
                        let name = var.lookup(self.db.upcast()).name(self.db.upcast());
                        let def =
                            var.lookup(self.db.upcast()).ast_ptr(self.db.upcast()).text_range();
                        let FileSpan { range, file } = parse.to_file_span(def, &src_map);
                        res.labels.push(Label {
                            style: LabelStyle::Secondary,
                            file_id: file,
                            range: range.into(),
                            message: format!("help: '{name}' was declared here"),
                        });
                        res.with_message(
                            "constant expressions must not contain variable references".to_owned(),
                        )
                    }
                }
            }

            /* Parameter */
            BodyDiagnostic::IllegalParamAccess { def, expr, param } => {
                let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);
                let (def_name, def_src) = self.lookup(def);
                let (ref_name, ref_src) = self.lookup(param);
                let def_span = parse.to_file_span(def_src, &src_map);
                let ref_span = parse.to_file_span(ref_src, &src_map);

                Report::error()
                    .with_message(format!(
                        "definition of '{def_name}' references parameter '{ref_name}' defined afterwards",
                    ))
                    .with_labels(vec![
                        Label {
                            style: LabelStyle::Primary,
                            file_id: file,
                            range: range.into(),
                            message: "illegal reference".to_owned(),
                        },
                        Label {
                            style: LabelStyle::Secondary,
                            file_id: def_span.file,
                            range: def_span.range.into(),
                            message: format!("help: '{def_name}' is defined here"),
                        },
                        Label {
                            style: LabelStyle::Secondary,
                            file_id: ref_span.file,
                            range: ref_span.range.into(),
                            message: format!(".. to parameter '{ref_name}' defined here"),
                        }
                    ])
                    .with_notes(vec![
                            "help: parameters may only refer to parameters (textually) defined before them"
                            .to_owned(),
                    ])
            }

            /* Function */
            BodyDiagnostic::WriteToInputArg { expr, arg } => {
                let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);
                let arg_name = arg.name(self.db.upcast());
                let arg_src = arg.ast_ptr(self.db.upcast()).text_range();
                let arg_src = parse.to_file_span(arg_src, &src_map);

                Report::error()
                    .with_message(format!("write to input function argument '{arg_name}'"))
                    .with_labels(vec![Label {
                        style: LabelStyle::Secondary,
                        file_id: arg_src.file,
                        range: arg_src.range.into(),
                        message: format!("help: '{arg_name}' is defined here"),
                    }])
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: file,
                        range: range.into(),
                        message: "write to input argument".to_owned(),
                    }])
                    .with_notes(vec![format!("help: change direction of '{arg_name}' to inout")])
            }
            BodyDiagnostic::UnsupportedFunction { expr, func } => {
                let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);

                Report::error()
                    .with_message(format!(
                        "function '{func:?}' is currently not supported by OpenVAF"
                    ))
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: file,
                        range: range.into(),
                        message: "unsupported function".to_owned(),
                    }])
                    .with_notes(vec![
                        "This function is part of the Verilog-A standard but currently not implemented by OpenVAF\n\
                        If this function is important to your application, create an issue:\n\
                        https://github.com/pascalkuthe/openvaf/issues/new".to_owned()
                    ])
            }
            BodyDiagnostic::ConstSimparam { known, expr, .. } => {
                let FileSpan { range, file } = self.expr_span(expr, &src_map, &parse);

                let mut report = Report::warning()
                    .with_message(
                        "call to $simparam in a constant is evaluated before simulation starts"
                            .to_owned(),
                    )
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: file,
                        range: range.into(),
                        message: "call to $simparam in a constant".to_owned(),
                    }]);
                if !known {
                    report = report.with_notes(vec![
                        "help: the value of parameters like \"gmin\" or \"sourceScaleFactor\" may vary between iterations"
                            .to_owned(),
                    ])
                }
                report
            }
        }
    }

    fn to_report(&self, root_file: FileId, db: &dyn BaseDB) -> Option<Report> {
        let mut report = self.build_report(root_file, db);

        if let Some((lint, lint_src)) = self.lint(root_file, db) {
            let (lvl, is_default) = match lint_src.overwrite {
                Some(lvl) => (lvl, false),
                None => db.lint_lvl(lint, root_file, lint_src.ast),
            };
            let LintData { name, documentation_id, .. } = db.lint_data(lint);

            report.code = Some(format!("L{:03}", documentation_id));
            report.severity = match lvl {
                LintLevel::Deny => Severity::Error,
                LintLevel::Warn => Severity::Warning,
                LintLevel::Allow => return None,
            };
            if is_default {
                let hint = format!("{name} is set to {lvl} by default");
                report.notes.push(hint)
            }
        }
        Some(report)
    }
}
