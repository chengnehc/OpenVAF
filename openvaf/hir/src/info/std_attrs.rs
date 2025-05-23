use super::*;

use stdx::impl_display;
use syntax::sourcemap::FileSpan;

use crate::diagnostics::{Diagnostic, Label, LabelStyle, Report};

/// Refer to [LRM 2.9.2] standard attributes
pub(super) enum StdAttrDiagnostic {
    IllegalAttr { attr: ast::Attr },
    UnknownParamType { attr: ast::Attr, found: String },
    UnknownMultiplicity { attr: ast::Attr, found: String },
}

use StdAttrDiagnostic::*;
impl_display! {
    match StdAttrDiagnostic {
        IllegalAttr { attr } => "illegal expression supplied to '{}' attribute; expected a string literal", attr.name().unwrap();
        UnknownParamType { found, .. } => r#"unknown parameter type "{}"; expected "model" or "instance""#, found;
        UnknownMultiplicity { found, .. } => r#"unknown multiplicity attribute value "{}"; expected "multiply", "divide" or "none""#, found;

    }
}

impl Diagnostic for StdAttrDiagnostic {
    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let src_map = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        let report = match self {
            IllegalAttr { attr } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::error().with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id: file,
                    range: range.into(),
                    message: "expected a string literal".to_owned(),
                }])
            }
            UnknownParamType { attr, .. } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::warning()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: file,
                        range: range.into(),
                        message: "unknown parameter type".to_owned(),
                    }])
                    .with_notes(
                        vec!["note: parameter type is set to 'model' by default".to_owned()],
                    )
            }
            UnknownMultiplicity { attr, .. } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::warning()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: file,
                        range: range.into(),
                        message: "unknown multiplicity attribute value".to_owned(),
                    }])
                    .with_notes(vec![
                        "note: multiplicity is set to 'none' by default, no scaling is performed"
                            .to_owned(),
                    ])
            }
        };

        report.with_message(self.to_string())
    }
}
