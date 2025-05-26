use codespan_reporting::diagnostic;
use syntax::sourcemap::{CtxSpan, FileSpan, SourceMap};
use syntax::{Parse, SourceFile, TextRange, TextSize};

pub use diagnostic::Severity;
pub type Report = diagnostic::Diagnostic<FileId>;
pub type Label = diagnostic::Label<FileId>;

use crate::lints::{self, Lint, LintData, LintLevel, LintSrc};
use crate::{BaseDB, FileId};

mod lint_attrs;
mod preprocess_error;
mod syntax_error;

pub mod sink;
pub use sink::{Buffer, ConsoleSink, DiagnosticSink};

pub trait Diagnostic {
    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report;

    fn lint(&self, _root_file: FileId, _db: &dyn BaseDB) -> Option<(Lint, LintSrc)> {
        None
    }

    fn to_report(&self, root_file: FileId, db: &dyn BaseDB) -> Option<Report> {
        let mut report = self.build_report(root_file, db);

        if let Some((lint, lint_src)) = self.lint(root_file, db) {
            let LintData { name, documentation_id, .. } = db.lint_data(lint);
            let (lvl, is_default) = lint_src.lvl(lint, root_file, db);

            report.code = Some(format!("L{:03}", documentation_id));
            report.severity = match lvl {
                LintLevel::Deny => Severity::Error,
                LintLevel::Warn => Severity::Warning,
                LintLevel::Allow => return None,
            };
            if is_default {
                let hint = format!(
                    "{name} is set to {lvl} by default\n\
                    use a CLI argument or an attribute to overwrite"
                );
                report.notes.push(hint)
            }
        }

        Some(report)
    }
}

pub const HINT_UNSUPPORTED: &str = "this is allowed by VerilogAMS language spec but was \
purposefully excluded from the supported language subset\n\
more details can be found in the OpenVAF documentation";

// TODO support (macro) expansion span backtrace

pub fn to_unified_spans<const N: usize>(
    sm: &SourceMap,
    mut spans: [CtxSpan; N],
) -> (FileId, [TextRange; N]) {
    assert!(N >= 2);
    let (file, ranges) = sm.to_file_spans(&mut spans);
    (file, ranges.try_into().unwrap())
}

pub fn to_unified_span_list(sm: &SourceMap, spans: &mut [CtxSpan]) -> (FileId, Vec<TextRange>) {
    match spans {
        [] => unimplemented!(),
        [span] => {
            let FileSpan { range, file } = span.to_file_span(sm);
            (file, vec![range])
        }
        spans => sm.to_file_spans(spans),
    }
}

pub fn text_ranges_to_unified_spans<const N: usize>(
    sm: &SourceMap,
    parse: &Parse<SourceFile>,
    ranges: [TextRange; N],
) -> (FileId, [TextRange; N]) {
    let spans = ranges.map(|range| parse.to_ctx_span(range, sm));
    to_unified_spans(sm, spans)
}

pub fn text_range_list_to_unified_spans(
    sm: &SourceMap,
    parse: &Parse<SourceFile>,
    ranges: &[TextRange],
) -> (FileId, Vec<TextRange>) {
    let mut spans: Vec<_> = ranges.iter().map(|range| parse.to_ctx_span(*range, sm)).collect();
    to_unified_span_list(sm, &mut spans)
}
