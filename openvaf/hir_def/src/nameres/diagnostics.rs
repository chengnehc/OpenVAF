use stdx::{impl_display, pretty};

use basedb::diagnostics::{Diagnostic, Label, Report};
use basedb::{BaseDB, FileId};
use syntax::name::Name;
use syntax::sourcemap::FileSpan;

use crate::db::HirDefDB;

use super::{ResolvedPath, ScopeItem};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathResolveError {
    NotFound { name: Name },
    NotFoundIn { name: Name, scope: Name },
    ExpectedScope { name: Name, found: ScopeItem },
    ExpectedItemKind { name: Name, expected: &'static str, found: ResolvedPath },
    ExpectedNatureAttrIdent { found: Box<[Name]> },
}

use PathResolveError::*;
impl_display! {
    match PathResolveError{
        NotFound{name} => "'{name}' was not found in the current scope";
        NotFoundIn{name, scope} => "'{name}' was not found in '{scope}'";
        ExpectedScope{name, found} => "expected a scope but found {} '{name}'", found.item_kind();
        ExpectedItemKind{name, expected, found} => "expected {} but found {} '{name}'", expected, found;
        ExpectedNatureAttrIdent{found} => "expected a nature attribute identifier but found path {}", pretty::List::path(found.clone());
    }
}

impl PathResolveError {
    pub fn message(&self) -> String {
        match self {
            NotFound { .. } | NotFoundIn { .. } => "not found".to_owned(),
            ExpectedScope { .. } | ExpectedNatureAttrIdent { .. } => {
                "failed to resolve path".to_owned()
            }
            ExpectedItemKind { expected, .. } => format!("expected {}", expected),
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum DefDiagnostic {
    AlreadyDeclared { old: ScopeItem, new: ScopeItem, name: Name },
}

impl_display! {
    match DefDiagnostic {
        DefDiagnostic::AlreadyDeclared{name, ..} => "'{name}' was already declared in this scope";
    }
}

// This wrapper is needed since the methods provided by `Diagnostic` trait
// takes argument with type `&dyn BaseDB` while `DefDiagnostic` requires data from
// `HirDefDB`.
//
// TODO(JW) can we make the `Diagnostic` trait accept upcast db like `HirDefDB`?
pub struct DefDiagnosticWrapped<'a> {
    pub db: &'a dyn HirDefDB,
    pub diag: &'a DefDiagnostic,
}

impl Diagnostic for DefDiagnosticWrapped<'_> {
    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let sm = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        let report = match self.diag {
            DefDiagnostic::AlreadyDeclared { old, new, name } => {
                let FileSpan { range, file } =
                    parse.to_file_span(new.text_range(self.db).unwrap(), &sm);
                let mut labels = vec![
                    Label::primary(file, range).with_message("already declared in this scope")
                ];
                if let Some(def) = old.text_range(self.db) {
                    let FileSpan { range, file } = parse.to_file_span(def, &sm);
                    labels.push(
                        Label::secondary(file, range)
                            .with_message(format!("help: '{name}' was first declared here")),
                    )
                }

                Report::error().with_labels(labels)
            }
        };

        report.with_message(self.diag)
    }
}
