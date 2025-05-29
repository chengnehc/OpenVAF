use syntax::sourcemap::FileSpan;

use crate::lints::builtin as builtins;
use crate::lints::LintAttrDiagnostic;

use super::*;

impl Diagnostic for LintAttrDiagnostic {
    fn lint(&self, _root_file: FileId, _db: &dyn BaseDB) -> Option<(Lint, LintSrc)> {
        match *self {
            Self::UnknownLint { src, .. } => Some((builtins::lint_not_found, LintSrc::item(src))),
            Self::LintOverwrite { src, .. } => {
                Some((builtins::lint_level_overwrite, LintSrc::item(src)))
            }
            _ => None,
        }
    }

    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let sm = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        let report = match *self {
            Self::ExpectedArrayOrLiteral { range, attr } => {
                let FileSpan { file, range } = parse.to_file_span(range, &sm);

                Report::error()
                    .with_label(
                        Label::primary(file, range).with_message("expected a literal or an array"),
                    )
                    .with_note(format!(
                        "help: valid examples are {0}=\"foo\" and {0}='{{\"foo\",\"bar\"}}",
                        attr
                    ))
            }
            Self::ExpectedLiteral { range, .. } => {
                let FileSpan { file, range } = parse.to_file_span(range, &sm);

                Report::error().with_label(
                    Label::primary(file, range).with_message("expected a string literal"),
                )
            }
            Self::LintOverwrite { old, new, .. } => {
                let (file, [new, old]) = text_ranges_to_unified_spans(&sm, &parse, [new, old]);

                Report::warning()
                    .with_labels(vec![
                        Label::secondary(file, old).with_message("lint lvl was first set here"),
                        Label::primary(file, new).with_message("lint lvl was overwritten her"),
                    ])
                    .with_note(
                        "help: the second lint lvl is used; the first attribute has no effect",
                    )
            }
            Self::UnknownLint { range, .. } => {
                let FileSpan { file, range } = parse.to_file_span(range, &sm);

                Report::error()
                    .with_label(Label::primary(file, range).with_message("unknown lint"))
                    .with_note("help: this attribute has no effect")
            }
        };

        report.with_message(self)
    }
}
