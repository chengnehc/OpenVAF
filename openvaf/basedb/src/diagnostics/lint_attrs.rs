use syntax::sourcemap::FileSpan;

use crate::lints::builtin as builtins;
use crate::lints::LintAttrDiagnostic::{self, *};

use super::*;

impl Diagnostic for LintAttrDiagnostic {
    fn lint(&self, _root_file: FileId, _db: &dyn BaseDB) -> Option<(Lint, LintSrc)> {
        match *self {
            UnknownLint { src, .. } => Some((builtins::lint_not_found, LintSrc::item(src))),
            LintOverwrite { src, .. } => Some((builtins::lint_level_overwrite, LintSrc::item(src))),
            _ => None,
        }
    }

    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let sm = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        let report = match *self {
            ExpectedArrayOrLiteral { range, attr } => {
                let FileSpan { file: file_id, range } = parse.to_file_span(range, &sm);
                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id,
                        range: range.into(),
                        message: "expected a literal or an array".to_owned(),
                    }])
                    .with_notes(vec![format!(
                        "help: valid examples are {0}=\"foo\" and {0}='{{\"foo\",\"bar\"}}",
                        attr
                    )])
            }
            ExpectedLiteral { range, .. } => {
                let FileSpan { file: file_id, range } = parse.to_file_span(range, &sm);
                Report::error().with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id,
                    range: range.into(),
                    message: "expected a string literal".to_owned(),
                }])
            }
            LintOverwrite { old, new, .. } => {
                let (file_id, [new, old]) = text_ranges_to_unified_spans(&sm, &parse, [new, old]);
                Report::warning()
                    .with_labels(vec![
                        Label {
                            style: LabelStyle::Secondary,
                            file_id,
                            range: old.into(),
                            message: "lint lvl was first set here".to_owned(),
                        },
                        Label {
                            style: LabelStyle::Primary,
                            file_id,
                            range: new.into(),
                            message: "lint lvl was overwritten her".to_owned(),
                        },
                    ])
                    .with_notes(vec![
                        "help: the second lint lvl is used; the first attribute has no effect"
                            .to_owned(),
                    ])
            }
            UnknownLint { range, .. } => {
                let FileSpan { file: file_id, range } = parse.to_file_span(range, &sm);
                Report::error()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id,
                        range: range.into(),
                        message: "unknown lint".to_owned(),
                    }])
                    .with_notes(vec!["help: this attribute has no effect".to_owned()])
            }
        };

        report.with_message(self.to_string())
    }
}
