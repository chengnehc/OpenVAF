use hir_def::{
    db::HirDefDB,
    nameres::{DefDiagnosticWrapped, DefMap, LocalScopeId, ScopeItemDef, ScopeOrigin},
    DefWithBodyId::{self, ModuleId},
    ItemTree,
};
use hir_ty::{
    inference::InferDiagnosticWrapped,
    validation::{BodyDiagnostic, BodyDiagnosticWrapped, TypeDiagnostic, TypeDiagnosticWrapped},
};

pub use basedb::diagnostics::*;
pub use basedb::{BaseDB, FileId};

use crate::{CompilationDB, HirDB};

/// Collect all diagnostics to `sink`
pub(crate) fn collect(db: &CompilationDB, root_file: FileId, sink: &mut impl DiagnosticSink) {
    // BaseDB
    sink.add_diagnostics(db.preprocess(root_file).errors(), root_file, db);
    sink.add_diagnostics(db.parse(root_file).errors().as_slice(), root_file, db);

    // HirDB (HirTyDB)
    let item_tree = db.item_tree(root_file);
    let def_map = db.root_def_map(root_file);

    // TODO(JW): does the order of collecting `def_map` and `type` diagnostics matter?
    collect_def_map(db, root_file, &def_map, sink);
    collect_type(db, root_file, &item_tree, sink);

    let root_scope = def_map.root_scope();
    for child in def_map[root_scope].children.values() {
        if let ScopeOrigin::Module(id) = def_map[*child].origin {
            // first, `analog initial` block
            collect_body(db, root_file, ModuleId { initial: true, id }, sink);
            // and then, normal `analog` block
            collect_body(db, root_file, ModuleId { initial: false, id }, sink)
        }
        collect_scope(db, root_file, &def_map, *child, sink)
    }
}

fn collect_def_map(
    db: &dyn HirDB,
    root_file: FileId,
    def_map: &DefMap,
    // parse: &Parse<SourceFile>,
    // sm: &SourceMap,
    // ast_id_map: &AstIdMap,
    sink: &mut impl DiagnosticSink,
) {
    for diag in &def_map.diagnostics {
        let diag = DefDiagnosticWrapped { db: db.upcast(), diag };
        sink.add_diagnostic(&diag, root_file, db.upcast())
    }
}

fn collect_scope(
    db: &CompilationDB,
    root_file: FileId,
    def_map: &DefMap,
    scope: LocalScopeId,
    // parse: &Parse<SourceFile>,
    // sm: &SourceMap,
    // ast_id_map: &AstIdMap,
    sink: &mut impl DiagnosticSink,
) {
    for def in def_map[scope].declarations.values() {
        if let Ok(def) = (*def).try_into() {
            collect_body(db, root_file, def, sink);
        }

        let def_map = match def {
            ScopeItemDef::FunctionId(fun) => db.function_def_map(*fun),
            ScopeItemDef::BlockId(block) => {
                if let Some(def_map) = db.block_def_map(*block) {
                    def_map
                } else {
                    continue;
                }
            }
            _ => continue,
        };

        collect_scope(db, root_file, &def_map, def_map.entry_scope(), sink);
        collect_def_map(db, root_file, &def_map, sink);
    }
}

fn collect_type(
    db: &dyn HirDB,
    root_file: FileId,
    // parse: &Parse<SourceFile>,
    // sm: &SourceMap,
    // ast_id_map: &AstIdMap,
    item_tree: &ItemTree,
    sink: &mut impl DiagnosticSink,
) {
    let diagnostics = TypeDiagnostic::collect(db, root_file);
    for diag in &diagnostics {
        let diag = TypeDiagnosticWrapped { db, diag, item_tree };
        sink.add_diagnostic(&diag, root_file, db.upcast());
    }
}

fn collect_body(
    db: &dyn HirDB,
    root_file: FileId,
    def: DefWithBodyId,
    // parse: &Parse<SourceFile>,
    // sm: &SourceMap,
    // ast_id_map: &AstIdMap,
    sink: &mut impl DiagnosticSink,
) {
    let body_sm = db.body_srcmap(def);

    let diagnostics = &db.inference_result(def).diagnostics;
    for diag in diagnostics {
        let diag = InferDiagnosticWrapped { db, diag, body_sm: &body_sm };
        let db: &dyn BaseDB = db.upcast();
        sink.add_diagnostic(&diag, root_file, db)
    }

    let diagnostics = BodyDiagnostic::validate_and_collect(db, def);
    for diag in &diagnostics {
        let diag = BodyDiagnosticWrapped { db, diag, body_sm: &body_sm };
        let db: &dyn BaseDB = db.upcast();
        sink.add_diagnostic(&diag, root_file, db)
    }
}
