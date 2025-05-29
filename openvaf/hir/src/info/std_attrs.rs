use super::*;

use stdx::impl_display;
use syntax::sourcemap::FileSpan;

use crate::diagnostics::{Diagnostic, Label, Report};

/// Refer to [LRM 2.9.2] standard attributes
pub(super) enum StdAttrDiagnostic {
    IllegalAttr { attr: ast::Attr },
    UnknownParamType { attr: ast::Attr, found: String },
    UnknownMultiplicity { attr: ast::Attr, found: String },
}

impl_display! {
    match StdAttrDiagnostic {
        Self::IllegalAttr { attr } =>
            "illegal expression supplied to '{}' attribute; expected a string literal", attr.name().unwrap();
        Self::UnknownParamType { found, .. } =>
            r#"unknown parameter type "{}"; expected "model" or "instance""#, found;
        Self::UnknownMultiplicity { found, .. } =>
            r#"unknown multiplicity attribute value "{}"; expected "multiply", "divide" or "none""#, found;

    }
}

impl Diagnostic for StdAttrDiagnostic {
    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let src_map = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        let report = match self {
            Self::IllegalAttr { attr } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::error().with_label(
                    Label::primary(file, range).with_message("expected a string literal"),
                )
            }
            Self::UnknownParamType { attr, .. } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::warning()
                    .with_label(Label::primary(file, range).with_message("unknown parameter type"))
                    .with_note("note: parameter type is set to 'model' by default")
            }
            Self::UnknownMultiplicity { attr, .. } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::warning()
                    .with_label(
                        Label::primary(file, range)
                            .with_message("unknown multiplicity attribute value"),
                    )
                    .with_note(
                        "note: multiplicity is set to 'none' by default, no scaling is performed",
                    )
            }
        };

        report.with_message(self)
    }
}
