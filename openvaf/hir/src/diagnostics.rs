use hir_def::nameres::{
    DefDiagnosticWrapped, DefMap, ItemWithBodyId, LocalScopeId, ScopeItem, ScopeOrigin,
};
use hir_ty::{
    inference::InferDiagnosticWrapped,
    validation::{BodyDiagnostic, BodyDiagnosticWrapped, TypeDiagnostic, TypeDiagnosticWrapped},
};

pub use basedb::diagnostics::*;
pub use basedb::{BaseDB, FileId};

use crate::{CompilationDB, HirDB, HirDefDB};

/// Collect all frontend diagnostics to `sink`.
pub(crate) fn collect(db: &CompilationDB, root_file: FileId, sink: &mut impl DiagnosticSink) {
    // BaseDB
    sink.add_diagnostics(db.preprocess(root_file).errors(), root_file, db);
    sink.add_diagnostics(&db.parse(root_file).errors(), root_file, db);

    // HirDB
    let def_map = db.root_def_map(root_file);
    let root_scope = def_map.root_scope();
    collect_def_diagnostics(db, root_file, &def_map, sink);
    collect_type_diagnostics(db, root_file, sink);

    use hir_def::nameres::ItemWithBodyId::ModuleId;
    for &child in def_map[root_scope].children().unwrap().values() {
        if let ScopeOrigin::Module(id) = def_map[child].origin() {
            collect_body_diagnostics(db, root_file, ModuleId { initial: true, id }, sink);
            collect_body_diagnostics(db, root_file, ModuleId { initial: false, id }, sink)
        }
        collect_scope_diagnostics(db, root_file, &def_map, child, sink)
    }
}

fn collect_def_diagnostics(
    db: &dyn HirDB,
    root_file: FileId,
    def_map: &DefMap,
    sink: &mut impl DiagnosticSink,
) {
    for diag in &def_map.diagnostics {
        let diag = DefDiagnosticWrapped { db: db.upcast(), diag };
        sink.add_diagnostic(&diag, root_file, db.upcast())
    }
}

fn collect_type_diagnostics(db: &dyn HirDB, root_file: FileId, sink: &mut impl DiagnosticSink) {
    let diagnostics = TypeDiagnostic::validate_and_collect(db, root_file);
    for diag in &diagnostics {
        let diag = TypeDiagnosticWrapped { db, diag };
        sink.add_diagnostic(&diag, root_file, db.upcast());
    }
}

fn collect_body_diagnostics(
    db: &dyn HirDB,
    root_file: FileId,
    item: ItemWithBodyId,
    sink: &mut impl DiagnosticSink,
) {
    let body_src_map = &db.body_src_map(item);

    // Inference
    for diag in &db.inference_result(item).diagnostics {
        let diag = InferDiagnosticWrapped { db, diag, body_src_map };
        sink.add_diagnostic(&diag, root_file, db.upcast())
    }

    // Body
    let diagnostics = BodyDiagnostic::validate_and_collect(db, item);
    for diag in &diagnostics {
        let diag = BodyDiagnosticWrapped { db, diag, body_src_map };
        sink.add_diagnostic(&diag, root_file, db.upcast())
    }
}

fn collect_scope_diagnostics(
    db: &CompilationDB,
    root_file: FileId,
    def_map: &DefMap,
    scope: LocalScopeId,
    sink: &mut impl DiagnosticSink,
) {
    for &item in def_map[scope].decls().values() {
        if let Ok(body) = item.try_into() {
            collect_body_diagnostics(db, root_file, body, sink);
        }
        let def_map = match item {
            ScopeItem::BlockId(block) => {
                let Some(def_map) = db.block_def_map(block) else { continue };
                def_map
            }
            ScopeItem::FunctionId(fun) => db.function_def_map(fun),
            _ => continue,
        };
        collect_def_diagnostics(db, root_file, &def_map, sink);
        collect_scope_diagnostics(db, root_file, &def_map, def_map.entry_scope(), sink);
    }
}
