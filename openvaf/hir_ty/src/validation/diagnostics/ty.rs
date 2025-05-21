use super::*;

use basedb::AstIdMap;
use hir_def::{DisciplineAttr, NatureAttr};

impl TypeDiagnosticWrapped<'_> {
    fn build_duplicate_item<Def, Item: Copy>(
        &self,
        info: &DuplicateItem<Item, Def>,
        mut to_span: impl FnMut(Item) -> FileSpan,
    ) -> Vec<Label> {
        let span = to_span(info.first);
        let mut labels = vec![Label {
            style: LabelStyle::Secondary,
            file_id: span.file,
            range: span.range.into(),
            message: "first declared here".to_owned(),
        }];
        let subsequent = info.subsequent.iter().map(|&item| {
            let span = to_span(item);
            Label {
                style: LabelStyle::Primary,
                file_id: span.file,
                range: span.range.into(),
                message: "redeclared here".to_owned(),
            }
        });
        labels.extend(subsequent);

        labels
    }
}

impl Diagnostic for TypeDiagnosticWrapped<'_> {
    fn lint(&self, _root_file: FileId, _db: &dyn BaseDB) -> Option<(Lint, LintSrc)> {
        match *self.diag {
            TypeDiagnostic::PortWithoutDirection { decl, .. } => {
                Some((lints::builtin::port_without_direction, LintSrc::item(decl)))
            }
            _ => None,
        }
    }

    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let src_map = db.sourcemap(root_file);
        let parse = db.parse(root_file);
        let ast_id_map = db.ast_id_map(root_file);

        match *self.diag {
            TypeDiagnostic::PathResolveError { ref err, src } => {
                let span = parse.to_file_span(src.text_range(), &src_map);

                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: span.file,
                        range: span.range.into(),
                        message: err.message(),
                    }])
                    .with_message(err.to_string())
            }

            TypeDiagnostic::DuplicateNatureAttr(ref info) => {
                let item_tree = self.db.item_tree(root_file);
                let db = self.db.upcast();
                let nature = &item_tree[info.src.lookup(db).id];
                let labels = self.build_duplicate_item(info, |attr| {
                    let id = u32::from(nature.attrs.start()) + u32::from(attr);
                    let id = NatureAttr::lookup(&item_tree, id.into()).ast_id();
                    parse.to_file_span(ast_id_map.get(id).text_range(), &src_map)
                });
                let attr_name = db.nature_data(info.src).attrs[info.first].name.clone();

                Report::error()
                    .with_message(format!(
                        "nature attribute '{attr_name}' was defined multiple times"
                    ))
                    .with_labels(labels)
            }
            TypeDiagnostic::DuplicateDisciplineAttr(ref info) => {
                let item_tree = self.db.item_tree(root_file);
                let db = self.db.upcast();
                let discipline = &item_tree[info.src.lookup(db).id];
                let labels = self.build_duplicate_item(info, |attr| {
                    let id = u32::from(discipline.attrs.start()) + u32::from(attr);
                    let id = DisciplineAttr::lookup(&item_tree, id.into()).ast_id();
                    parse.to_file_span(ast_id_map.get(id).text_range(), &src_map)
                });
                let attr_name = db.discipline_data(info.src).attrs[info.first].name.clone();

                Report::error()
                    .with_message(format!(
                        "discipline attribute '{attr_name}' was defined multiple times"
                    ))
                    .with_labels(labels)
            }

            TypeDiagnostic::PortWithoutDirection { decl, ref name } => {
                let span = parse.to_file_span(ast_id_map.get_erased(decl).text_range(), &src_map);

                Report::error()
                .with_message(format!("no direction declared for port '{name}'"))
                .with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id: span.file,
                    range: span.range.into(),
                    message: format!("'{name}' is declared here without direction"),
                }])
                .with_notes(vec![
                    "if port_without_direction is set to warn/allow the direciton will be set to 'inout'.".to_owned(), 
                    "note: port directions are always required by the language standard.".to_owned()])
            }
            TypeDiagnostic::NodeWithoutDiscipline { decl, ref name } => {
                let span = parse.to_file_span(ast_id_map.get_erased(decl).text_range(), &src_map);

                Report::error()
                .with_message(format!("no discipline for net '{name}'"))
                .with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id: span.file,
                    range: span.range.into(),
                    message: format!("'{name}' is missing a discipline"),
                }])
                .with_notes(vec![
                    "info: disciplineless nets are digital and therefore not supported in Verilog-A".to_owned(),
                    format!("help: add a discipline with 'electrical {name}'"),
                ])
            }
            TypeDiagnostic::MultipleDirections(ref info) => {
                let labels = self.build_duplicate_item(info, |id| {
                    parse.to_file_span(ast_id_map.get_erased(id).text_range(), &src_map)
                });
                let node_name = self.db.node_data(info.src).name.clone();

                Report::error()
                    .with_message(format!("multiple direction declarations for port '{node_name}'"))
                    .with_labels(labels)
            }
            TypeDiagnostic::MultipleDisciplines(ref info) => {
                let labels = self.build_duplicate_item(info, |id| {
                    parse.to_file_span(ast_id_map.get_erased(id).text_range(), &src_map)
                });
                let node_name = self.db.node_data(info.src).name.clone();

                Report::error()
                    .with_message(format!("multiple discipline declarations for net '{node_name}'"))
                    .with_labels(labels)
            }
            TypeDiagnostic::MultipleGnds(ref info) => {
                let labels = self.build_duplicate_item(info, |id| {
                    parse.to_file_span(ast_id_map.get_erased(id).text_range(), &src_map)
                });
                let node_name = self.db.node_data(info.src).name.clone();

                Report::error()
                    .with_message(format!("multiple 'ground' declarations for net '{node_name}'"))
                    .with_labels(labels)
            }

            TypeDiagnostic::ExpectedPort { node, src } => {
                let span = parse.to_file_span(ast_id_map.get_erased(src).text_range(), &src_map);
                let decl = parse.to_file_span(
                    node.lookup(self.db.upcast()).ast_ptr(self.db.upcast()).text_range(),
                    &src_map,
                );
                let node_name = &self.db.node_data(node).name;

                Report::error()
                    .with_message(format!(
                        "expected a port reference but no direction was declared for net '{node_name}"
                    ))
                    .with_labels(vec![
                        Label {
                            style: LabelStyle::Primary,
                            file_id: span.file,
                            range: span.range.into(),
                            message: format!("'{node_name}' is not a port"),
                        },
                        Label {
                            style: LabelStyle::Secondary,
                            file_id: decl.file,
                            range: decl.range.into(),
                            message: format!("info: '{node_name}' was declared here"),
                        },
                    ])
                    .with_notes(vec![
                        "help: prefix one of the declarations with inout, input or output"
                            .to_owned(),
                    ])
            }
            TypeDiagnostic::IncompatibleBranch { branch, node1, node2 } => {
                let db = self.db.upcast();
                let branch = branch.lookup(db);
                let branch_range = branch.ast_ptr(db).text_range();
                let branch_name = branch.name(db).to_string();

                IncompatibleBranchDiagnostic {
                    branch_span: parse.to_file_span(branch_range, &src_map),
                    branch_name,
                    node1,
                    node2,
                }
                .into_report(self.db, &parse, &ast_id_map, &src_map)
            }

            TypeDiagnostic::MultipleFuncArgBind(ref info) => {
                let labels = self.build_duplicate_item(info, |id| {
                    parse.to_file_span(ast_id_map.get(id).text_range(), &src_map)
                });

                Report::error()
                    .with_message(format!(
                        "function argument '{}' is binded with multiple variable declarations",
                        info.src
                    ))
                    .with_labels(labels)
            }
            TypeDiagnostic::FuncArgWithoutVarBind { decl, ref name } => {
                let span = parse.to_file_span(ast_id_map.get(decl).text_range(), &src_map);

                Report::error()
                    .with_message(format!("argument '{name}' is not binded with any variable"))
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: span.file,
                        range: span.range.into(),
                        message: format!("'{name}' shall have an associated variable declaration"),
                    }])
            }
        }
    }
}

impl IncompatibleBranchDiagnostic {
    pub(super) fn into_report(
        self,
        db: &dyn HirTyDB,
        parse: &Parse<SourceFile>,
        ast_id_map: &AstIdMap,
        src_map: &SourceMap,
    ) -> Report {
        let Self { branch_span, branch_name, node1, node2 } = self;

        let id1 = node1.lookup(db.upcast()).ast_id(db.upcast()).unwrap();
        let range1 = ast_id_map.get_erased(id1).text_range();
        let span1 = parse.to_file_span(range1, src_map);

        let id2 = node2.lookup(db.upcast()).ast_id(db.upcast()).unwrap();
        let range2 = ast_id_map.get_erased(id2).text_range();
        let span2 = parse.to_file_span(range2, src_map);

        let node1 = db.node_data(node1);
        let node2 = db.node_data(node2);

        let msg = format!(
            "nodes '{}' and '{}' of branch '{}' have incompatible disciplines",
            node1.name, node2.name, branch_name
        );

        Report::error()
            .with_message(msg)
            .with_labels(vec![
                Label {
                    style: LabelStyle::Primary,
                    file_id: branch_span.file,
                    range: branch_span.range.into(),
                    message: format!("branch '{}' has mismatched disciplines", branch_name),
                },
                Label {
                    style: LabelStyle::Secondary,
                    file_id: span1.file,
                    range: span1.range.into(),
                    message: format!("help: '{}' declared with discipline '{}'", node1.name, node1.discipline.as_ref().unwrap()),
                },
                Label {
                    style: LabelStyle::Secondary,
                    file_id: span2.file,
                    range: span2.range.into(),
                    message: format!("help: '{}' declared with discipline '{}'", node2.name, node2.discipline.as_ref().unwrap()),
                }
            ])
            .with_notes(vec![
                "help: disciplines are compatible if their potential and flow natures have the same 'units' attribute".to_owned()
            ])
    }
}
