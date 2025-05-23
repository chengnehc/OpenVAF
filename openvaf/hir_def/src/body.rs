use std::sync::Arc;

use ahash::AHashMap as HashMap;
use arena::{Arena, ArenaMap};
use basedb::lints::{Lint, LintAttrDiagnostic, LintAttrs, LintSrc};
use syntax::{ast, AstPtr};

use crate::db::HirDefDB;
use crate::item_tree::{DisciplineAttr, ItemTreeId, NatureAttr};
use crate::nameres::{DefMapSource, ItemWithBodyId, LocalScopeId};
use crate::{DisciplineAttrLoc, DisciplineLoc, NatureAttrLoc, NatureLoc};
use crate::{Lookup, ScopeId, Type};

mod expr;
mod lower;
mod param;
mod pretty;

pub use expr::*;
pub use param::{ConstraintValue, ParamConstraint, ParamExprs};

/// The body of an item that contains HIR-level expressions and statements.
#[derive(Debug, Eq, PartialEq, Default)]
pub struct Body {
    pub exprs: Arena<Expr>,
    pub stmts: Arena<Stmt>,
    pub entry_stmts: Box<[StmtId]>,
    pub stmt_scopes: ArenaMap<Stmt, ScopeId>,
}

/// The mapping from AST node positions to HIR expression/statement IDs (and in reverse).
/// This is needed to go from e.g. a position in a file to the HIR expression/statement
/// containing it.
///
/// But for type inference etc., we want to operate on a structure that is agnostic to
/// the actual positions of expressions in the file, so that we don't recompute types
/// whenever some whitespace is typed.
#[derive(Default, Debug, Eq, PartialEq)]
pub struct BodySourceMap {
    /// from AST to HIR
    pub expr_map: HashMap<AstPtr<ast::Expr>, ExprId>,
    /// from HIR to AST, for desugared expr, the value is `None`
    pub expr_map_back: ArenaMap<Expr, Option<AstPtr<ast::Expr>>>,
    /// from AST to HIR
    pub stmt_map: HashMap<AstPtr<ast::Stmt>, StmtId>,
    /// from HIR to AST, for desugared stmt, the value is `None`
    pub stmt_map_back: ArenaMap<Stmt, Option<AstPtr<ast::Stmt>>>,
    /// for user-defined lint attributes
    lint_map: ArenaMap<Stmt, LintAttrs>,
    /// Diagnostics accumulated during body lowering.
    /// These contain `AstPtr`s and so are stored in the source map (since they're just as volatile).
    pub diagnostics: Vec<LintAttrDiagnostic>,
}

impl BodySourceMap {
    pub fn lint_src(&self, stmt: StmtId, lint: Lint) -> LintSrc {
        self.lint_map[stmt].lint_src(lint)
    }
}

impl Body {
    pub fn body_with_srcmap_query(
        db: &dyn HirDefDB,
        def: ItemWithBodyId,
    ) -> (Arc<Body>, Arc<BodySourceMap>) {
        let mut body = Body::default();
        let mut src_map = BodySourceMap::default();

        let root_file = def.file(db);
        let ast_id_map = &db.ast_id_map(root_file);

        match def {
            ItemWithBodyId::NatureAttrId(attr) => {
                let root = db.parse(root_file).root();
                let item_tree = db.item_tree(root_file);

                let NatureAttrLoc { nature, id } = attr.lookup(db);
                let NatureLoc { root_file, id: nature_id } = nature.lookup(db);

                let nature = &item_tree[nature_id];
                let idx = usize::from(nature.attrs.start()) + usize::from(id);
                let attr = &item_tree[ItemTreeId::<NatureAttr>::from(idx)];
                let ast_node = ast_id_map.get(attr.ast_id).to_node(&root);
                let curr_scope = (ScopeId::root(root_file), attr.ast_id.into());
                let mut ctxt = lower::Context {
                    body: &mut body,
                    src_map: &mut src_map,
                    db,
                    ast_id_map,
                    curr_scope,
                };
                let expr = ctxt.collect_expr_opt(ast_node.val());
                let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(expr));
                body.entry_stmts = Box::from([stmt]);
            }
            ItemWithBodyId::DisciplineAttrId(attr) => {
                let root = db.parse(root_file).root();
                let item_tree = db.item_tree(root_file);

                let DisciplineAttrLoc { discipline, id } = attr.lookup(db);
                let DisciplineLoc { root_file, id: discipline_id } = discipline.lookup(db);

                let discipline = &item_tree[discipline_id];
                let idx = usize::from(discipline.attrs.start()) + usize::from(id);
                let attr = &item_tree[ItemTreeId::<DisciplineAttr>::from(idx)];
                let ast_node = ast_id_map.get(attr.ast_id).to_node(&root);
                let curr_scope = (ScopeId::root(root_file), attr.ast_id.into());
                let mut ctxt = lower::Context {
                    body: &mut body,
                    src_map: &mut src_map,
                    db,
                    ast_id_map,
                    curr_scope,
                };
                let expr = ctxt.collect_expr_opt(ast_node.val());
                let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(expr));
                body.entry_stmts = Box::from([stmt]);
            }
            ItemWithBodyId::ModuleId { initial, id } => {
                let module = id.lookup(db);
                let ast_id = module.ast_id(db);
                let ast_node = module.source(db);
                let mut ctxt = lower::Context {
                    body: &mut body,
                    src_map: &mut src_map,
                    db,
                    ast_id_map,
                    curr_scope: (module.scope, ast_id.into()),
                };
                // concatenate all the analog behavioral blocks
                body.entry_stmts = if initial {
                    ast_node
                        .analog_initial_behaviors()
                        .map(|stmt| ctxt.collect_stmt(stmt))
                        .collect()
                } else {
                    ast_node.analog_behaviors().map(|stmt| ctxt.collect_stmt(stmt)).collect()
                };
            }
            ItemWithBodyId::VarId(id) => {
                let var = id.lookup(db);
                let ast_id = var.ast_id(db);
                let ast_node = var.source(db);
                let mut ctxt = lower::Context {
                    body: &mut body,
                    src_map: &mut src_map,
                    db,
                    ast_id_map,
                    curr_scope: (var.scope, ast_id.into()),
                };
                let expr = if let Some(expr) = ast_node.initial() {
                    // the variable is explicitly initialized
                    ctxt.collect_expr(expr)
                } else {
                    // initialize the variable with zero if it is not initialized
                    let default_val = match db.var_data(id).ty {
                        Type::Real => Literal::Float(0.0f64.into()),
                        Type::Integer => Literal::Int(0),
                        Type::String => Literal::String(String::new().into_boxed_str()),
                        _ => unreachable!("invalid var type (TODO arrays)"),
                    };
                    ctxt.alloc_expr_desugared(Expr::Literal(default_val))
                };
                // allocate an expr stmt as the entry stmt of variable body
                let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(expr));
                body.entry_stmts = Box::from([stmt]);
            }
            ItemWithBodyId::ParamId(param) => {
                let (body, sm, _) = db.param_body_with_srcmap(param);
                return (body, sm);
            }
            ItemWithBodyId::FunctionId(id) => {
                let scope =
                    ScopeId::from(root_file, DefMapSource::Function(id), LocalScopeId::from(0u32));
                debug_assert_eq!(scope.id, db.function_def_map(id).entry_scope());

                let fun = id.lookup(db);
                let ast_id = fun.ast_id(db);
                let ast_node = fun.source(db);
                let mut ctxt = lower::Context {
                    body: &mut body,
                    src_map: &mut src_map,
                    db,
                    ast_id_map,
                    curr_scope: (scope, ast_id.into()),
                };
                body.entry_stmts = ast_node.body().map(|stmt| ctxt.collect_stmt(stmt)).collect();
            }
        }

        (Arc::new(body), Arc::new(src_map))
    }
}
