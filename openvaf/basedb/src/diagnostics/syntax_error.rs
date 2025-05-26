use std::iter::{once, zip};

use codespan_reporting::diagnostic::Label;
use syntax::{ast, AstNode, SyntaxError};

use super::*;

fn syntax_err_report(missing_delimiter: bool) -> Report {
    if missing_delimiter {
        Report::error().with_note("you might be missing a 'begin' delimiter")
    } else {
        Report::error()
    }
}

impl Diagnostic for SyntaxError {
    fn lint(&self, root_file: FileId, db: &dyn BaseDB) -> Option<(Lint, LintSrc)> {
        let map = db.ast_id_map(root_file);
        match self {
            SyntaxError::ReservedIdentifier { compat: true, src, .. } => Some((
                lints::builtin::vams_keyword_compat,
                LintSrc { overwrite: None, ast: map.nearest_ast_id_to(*src, db, root_file) },
            )),
            _ => None,
        }
    }

    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let sm = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        let report = match *self {
            SyntaxError::UnexpectedToken {
                ref expected,
                range,
                expected_at: Some(expected_at),
                missing_delimiter,
                panic_end: None,
                ..
            } => {
                let (file, [expected_at, range]) =
                    text_ranges_to_unified_spans(&sm, &parse, [expected_at, range]);

                syntax_err_report(missing_delimiter).with_labels(vec![
                    Label::primary(file, range).with_message("unexpected token"),
                    Label::secondary(file, expected_at)
                        .with_message(format!("expected {}", expected)),
                ])
            }
            SyntaxError::UnexpectedToken {
                ref expected,
                range,
                missing_delimiter,
                panic_end: Some(panic_end),
                ..
            } => {
                let message = if expected.data.len() < 4 {
                    format!("expected {}", expected)
                } else {
                    "unexpected_token".to_owned()
                };
                let (file, [range, skipped]) = text_ranges_to_unified_spans(
                    &sm,
                    &parse,
                    [range, TextRange::new(range.start(), panic_end)],
                );

                syntax_err_report(missing_delimiter).with_labels(vec![
                    Label::primary(file, range).with_message(message),
                    Label::secondary(file, usize::from(range.end())..usize::from(skipped.end()))
                        .with_message("skipping to next valid declaration"),
                ])
            }
            SyntaxError::UnexpectedToken { ref expected, range, missing_delimiter, .. } => {
                let message = if expected.data.len() < 4 {
                    format!("expected {}", expected)
                } else {
                    "unexpected_token".to_owned()
                };
                let FileSpan { file, range } = parse.to_file_span(range, &sm);

                syntax_err_report(missing_delimiter)
                    .with_label(Label::primary(file, range).with_message(message))
            }

            SyntaxError::ReservedIdentifier { src, compat, ref name } => {
                let FileSpan { range, file } = parse.to_file_span(src.text_range(), &sm);

                let report = Report::error().with_label(
                    Label::primary(file, range).with_message(format!("'{name}' is a keyword")),
                );
                // TODO error code (doc)
                if compat {
                    report.with_note(format!(
                        "'{name}' will likely never be used in the implemented language subset so this use is allowed",
                        ))
                        .with_note(
                        "to maintain compatibility with the VAMS standard this should be renamed"
                    )
                } else {
                    report
                }
            }

            SyntaxError::IllegalRootSegment { path_segment, prefix: None } => {
                let FileSpan { file, range } = parse.to_file_span(path_segment, &sm);
                let end = TextRange::at(range.end() - TextSize::from(1), 1.into());

                Report::error().with_labels(vec![
                    Label::primary(file, range).with_message("'$root' must be a prefix"),
                    Label::secondary(file, end).with_message(".<identifier> might be missing here"),
                ])
            }
            SyntaxError::IllegalRootSegment { path_segment, prefix: Some(prefix) } => {
                let (file, [prefix, path_segment]) =
                    text_ranges_to_unified_spans(&sm, &parse, [prefix, path_segment]);
                let prefix = TextRange::at(prefix.start() - TextSize::from(1), 1.into());

                Report::error().with_labels(vec![
                    Label::primary(file, path_segment).with_message("'$root' must be a prefix"),
                    Label::secondary(file, prefix)
                        .with_message("perhaps you meant to place '$root' here"),
                ])
            }

            SyntaxError::IllegalNatureIdent { range } => {
                let FileSpan { range, file } = parse.to_file_span(range, &sm);

                Report::error()
                    .with_label(
                        Label::primary(file, range).with_message("illegal nature identifier"),
                    )
                    .with_note(
                        "help: expected one of the following\n\
                        - an identifier: 'Voltage'\n\
                        - an identifier preceded by a discipline: 'electrical.potential'",
                    )
            }
            SyntaxError::IllegalAttribute { expected, range, .. } => {
                let FileSpan { range, file } = parse.to_file_span(range, &sm);

                Report::error().with_label(
                    Label::primary(file, range).with_message(format!("expected {}", expected)),
                )
            }

            SyntaxError::SurplusToken { found, range } => {
                let FileSpan { file, range } = parse.to_file_span(range, &sm);

                Report::error()
                    .with_label(
                        Label::primary(file, range).with_message(format!("unexpected {}", found)),
                    )
                    .with_note(format!("the {found} token is not required here; simply remove it",))
            }
            SyntaxError::MissingToken { expected, range, expected_at } => {
                let (file, [expected_at, range]) =
                    text_ranges_to_unified_spans(&sm, &parse, [expected_at, range]);

                Report::error().with_labels(vec![
                    Label::primary(file, range).with_message("unexpected token"),
                    Label::secondary(file, expected_at)
                        .with_message(format!("{} might be missing here", expected)),
                ])
            }
            SyntaxError::IllegalDisciplineAttrPath { range } => {
                let FileSpan { range, file } = parse.to_file_span(range, &sm);

                Report::error()
                    .with_label(Label::primary(file, range).with_message("illegal attribute path"))
                    .with_note(
                        "help: expected a nature attribute preceded by 'potential' or 'flow': potential.abstol"
                    )
            }

            SyntaxError::IllegalInfToken { range } => {
                let FileSpan { range, file } = parse.to_file_span(range, &sm);

                Report::error().with_label(Label::primary(file, range).with_message("unexpected token"))
                .with_note("help: 'inf' is only allowed in ranges of parameter declarations (example: [0:inf))")
            }

            SyntaxError::IllegalBodyPorts { head, ref body_ports } => {
                let body_ports = body_ports.iter().map(|&range| parse.to_file_span(range, &sm));
                let head = parse.to_file_span(head, &sm);

                let mut labels: Vec<_> = body_ports
                    .map(|span| {
                        Label::primary(span.file, span.range)
                            .with_message("illegal port declaration")
                    })
                    .collect();
                labels.push(
                    Label::secondary(head.file, head.range)
                        .with_message("info: ports already declared in header..."),
                );

                Report::error().with_labels(labels).with_notes(vec![
                    "help: either place all port declaration in the header".to_owned(),
                    "or place all port declarations in the body".to_owned(),
                ])
            }
            SyntaxError::PortNotDeclaredInModuleHead { head, pos, ref name } => {
                let pos = parse.to_file_span(pos, &sm);
                let head = parse.to_file_span(head, &sm);

                Report::error().with_labels(vec![
                    Label::primary(pos.file, pos.range)
                        .with_message("port not declared in module head"),
                    Label::secondary(head.file, head.range)
                        .with_message(format!("help: add {name} here")),
                ])
            }
            SyntaxError::MixedModuleHead { ref module_ports } => {
                let ports = module_ports.to_node(&parse.root()).ports();
                let names = ports.clone().filter_map(|port| port.name());
                let decls = ports.filter_map(|port| port.decl());

                let name_cnt = names.clone().count();
                let ranges: Vec<_> = names
                    .map(|name| name.syntax().text_range())
                    .chain(decls.map(|decl| decl.syntax().text_range()))
                    .collect();

                let (file, ranges) = text_range_list_to_unified_spans(&sm, &parse, &ranges);
                let names = &ranges[..name_cnt];
                let ports = &ranges[name_cnt..];

                let labels: Vec<_> = names
                    .iter()
                    .map(|&range| {
                        Label::secondary(file, range).with_message("found reference here")
                    })
                    .chain(ports.iter().map(|&range| {
                        Label::primary(file, range).with_message("port declaration not allowed")
                    }))
                    .collect();

                Report::error().with_labels(labels).with_notes(vec![
                    "either declare all ports directly in the header: module example(inout foo, inout bar);".to_owned(),
                    "or only reference ports in the header: module example(foo, bar);".to_owned(),
                ])
            }
            SyntaxError::DuplicatePort { ref pos, ref name } => {
                let mut spans = pos.iter().map(|range| parse.to_file_span(*range, &sm));
                let initial = spans.nth(0).unwrap();

                let mut labels: Vec<_> = spans
                    .map(|span| {
                        Label::primary(span.file, span.range).with_message("..redeclared here")
                    })
                    .collect();
                labels.push(
                    Label::secondary(initial.file, initial.range)
                        .with_message(format!("{name} first declared here")),
                );

                Report::error().with_labels(labels)
            }

            SyntaxError::IllegalBranchNodeCnt { arg_list, .. } => {
                let FileSpan { range, file } = parse.to_file_span(arg_list, &sm);

                Report::error()
                    .with_label(Label::primary(file, range).with_message("expected 1 or 2 nets"))
            }
            SyntaxError::IllegalBranchNodeExpr { single, ref illegal_nodes } => {
                let (file, illegal_nodes) =
                    text_range_list_to_unified_spans(&sm, &parse, illegal_nodes);

                let labels = illegal_nodes
                    .into_iter()
                    .map(|range| Label::primary(file, range).with_message("unexpected expression"))
                    .collect();

                let hint = if single {
                    "help: expected an identifier or a port flow expression (<port>)"
                } else {
                    "help: expected an identifier"
                };

                Report::error().with_labels(labels).with_note(hint)
            }

            SyntaxError::IllegalNetType { range, .. } => {
                let FileSpan { range, file } = parse.to_file_span(range, &sm);

                Report::error()
                    .with_label(Label::primary(file, range).with_message("unsupported net type"))
            }

            SyntaxError::RangeConstraintForNonNumericParameter { range, ty, .. } => {
                let (file, [range, ty]) = text_ranges_to_unified_spans(&sm, &parse, [range, ty]);

                Report::error().with_labels(vec![
                    Label::primary(file, range).with_message("illegal range bounds"),
                    Label::secondary(file, ty).with_message("help: expected real or integer"),
                ])
            }

            SyntaxError::BlockDeclsAfterStmt { decls: ref items, first_stmt } => {
                let ranges: Vec<_> =
                    once(first_stmt).chain(items.iter().map(|item| item.text_range())).collect();
                let (file, ranges) = text_range_list_to_unified_spans(&sm, &parse, &ranges);

                let first = ranges[0];
                let item_ranges = &ranges[1..];

                let mut labels: Vec<_> = zip(item_ranges, items)
                    .map(|(&range, item)| {
                        Label::primary(file, range).with_message(format!(
                            "{}s are only allowed before the first stmt",
                            item.syntax_kind()
                        ))
                    })
                    .collect();
                labels.push(
                    Label::secondary(file, first)
                        .with_message("help: move all declarations before this statement"),
                );

                Report::error().with_labels(labels)
            }
            SyntaxError::BlockDeclsWithoutScope { decls: ref items, begin_token } => {
                let ranges: Vec<_> =
                    once(begin_token).chain(items.iter().map(|item| item.text_range())).collect();
                let (file, ranges) = text_range_list_to_unified_spans(&sm, &parse, &ranges);

                let begin_token = TextRange::at(ranges[0].end() - TextSize::from(1), 1.into());
                let item_ranges = &ranges[1..];

                let mut labels: Vec<_> = zip(item_ranges, items)
                    .map(|(&range, item)| {
                        Label::primary(file, range)
                            .with_message(format!("{}s require a scope", item.syntax_kind()))
                    })
                    .collect();
                labels.push(
                    Label::secondary(file, begin_token).with_message("help: add ':<scope>' here"),
                );

                Report::error().with_labels(labels)
            }

            SyntaxError::FuncWithoutBody { fun } => {
                let FileSpan { range, file } = parse.to_file_span(fun, &sm);

                Report::error().with_label(
                    Label::primary(file, range).with_message("function body is missing"),
                )
            }
            SyntaxError::FuncWithoutArg { fun } => {
                let FileSpan { range, file } = parse.to_file_span(fun, &sm);

                Report::error().with_label(
                    Label::primary(file, range)
                        .with_message("function shall have at least one formal argument"),
                )
            }
            SyntaxError::ItemsAfterFuncBody { ref items, body } => {
                let ranges: Vec<_> =
                    once(body).chain(items.iter().map(|item| item.text_range())).collect();
                let (file, ranges) = text_range_list_to_unified_spans(&sm, &parse, &ranges);

                let body = ranges[0];
                let item_ranges = &ranges[1..];

                let mut labels: Vec<_> = zip(item_ranges, items)
                    .map(|(&range, item)| {
                        Label::primary(file, range).with_message(format!(
                            "{}s are not allowed after the function body",
                            item.syntax_kind()
                        ))
                    })
                    .collect();
                labels.push(
                    Label::secondary(file, body)
                        .with_message("help: move all declarations before this statement"),
                );

                Report::error().with_labels(labels)
            }
            SyntaxError::MultipleFuncBodies { ref additional_bodies, ref body } => {
                let (range, message) = if ast::BlockStmt::can_cast(body.syntax_kind()) {
                    (body.text_range(), "help: add these statements to this block")
                } else {
                    (
                        body.text_range().cover(*additional_bodies.last().unwrap()),
                        "help: surround with begin ... end to create a single function body",
                    )
                };

                let ranges: Vec<_> = once(range).chain(additional_bodies.iter().copied()).collect();
                let (file, ranges) = text_range_list_to_unified_spans(&sm, &parse, &ranges);
                let range = ranges[0];
                let item_ranges = &ranges[1..];

                let mut labels: Vec<_> = item_ranges
                    .iter()
                    .map(|&range| {
                        Label::primary(file, range)
                            .with_message("only one body per function is allowed")
                    })
                    .collect();
                labels.push(Label::secondary(file, range).with_message(message));

                Report::error().with_labels(labels)
            }
            SyntaxError::NamedFuncBodyBlock { name: scope } => {
                let FileSpan { range, file } = parse.to_file_span(scope, &sm);

                Report::error()
                    .with_label(Label::primary(file, range).with_message("unexpected block name"))
                    .with_note("help: remove the block scope name")
            }
        };

        report.with_message(self)
    }
}
