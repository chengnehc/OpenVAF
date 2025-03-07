use std::sync::Arc;
use stdx::Ieee64;

use ahash::AHashMap as HashMap;
use arena::{Arena, ArenaMap};
use basedb::lints::{Lint, LintSrc};
use basedb::{AttrDiagnostic, LintAttrs};
use syntax::{ast, AstNode, AstPtr};

use crate::db::HirDefDB;
use crate::item_tree::{DisciplineAttr, ItemTreeId, NatureAttr};
use crate::nameres::{DefMapSource, LocalScopeId};
use crate::{
    DefWithBodyId, DisciplineAttrLoc, DisciplineLoc, Expr, ExprId, Literal, Lookup, NatureAttrLoc,
    NatureLoc, ParamId, ScopeId, Stmt, StmtId, Type,
};

mod lower;
mod pretty;

/// The body of an item (functions, analog block, etc.)
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
    pub expr_map: HashMap<AstPtr<ast::Expr>, ExprId>,
    pub expr_map_back: ArenaMap<Expr, Option<AstPtr<ast::Expr>>>,
    pub stmt_map: HashMap<AstPtr<ast::Stmt>, StmtId>,
    pub stmt_map_back: ArenaMap<Stmt, Option<AstPtr<ast::Stmt>>>,
    /// for user-defined lint attributes
    lint_map: ArenaMap<Stmt, LintAttrs>,
    /// Diagnostics accumulated during body lowering. These contain `AstPtr`s and so are stored in
    /// the source map (since they're just as volatile).
    pub diagnostics: Vec<AttrDiagnostic>,
}

impl BodySourceMap {
    pub fn lint_src(&self, stmt: StmtId, lint: Lint) -> LintSrc {
        self.lint_map[stmt].lint_src(lint)
    }
}

impl Body {
    pub fn body_with_sourcemap_query(
        db: &dyn HirDefDB,
        def: DefWithBodyId,
    ) -> (Arc<Body>, Arc<BodySourceMap>) {
        let mut body = Body::default();
        let mut src_map = BodySourceMap::default();

        let root_file = def.file(db);
        let ast_id_map = db.ast_id_map(root_file);
        let registry = &db.lint_registry();

        match def {
            DefWithBodyId::ParamId(param) => {
                let (body, sm, _) = db.param_body_with_sourcemap(param);
                return (body, sm);
            }
            DefWithBodyId::ModuleId { initial, id } => {
                let module = id.lookup(db);
                let ast_id = module.ast_id(db);
                let ast_node = module.source(db);
                let curr_scope = (module.scope, ast_id.into());
                let mut ctxt = lower::Context {
                    db,
                    src_map: &mut src_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry,
                };

                body.entry_stmts = if initial {
                    ast_node
                        .analog_initial_behaviour()
                        .map(|stmt| ctxt.collect_stmt(stmt))
                        .collect()
                } else {
                    ast_node.analog_behaviour().map(|stmt| ctxt.collect_stmt(stmt)).collect()
                };
            }
            DefWithBodyId::FunctionId(id) => {
                let scope = ScopeId {
                    root_file,
                    local_id: LocalScopeId::from(0u32),
                    src: DefMapSource::Function(id),
                };
                debug_assert_eq!(scope.local_id, db.function_def_map(id).entry_scope());

                let fun = id.lookup(db);
                let ast_id = fun.ast_id(db);
                let ast_node = fun.source(db);
                let curr_scope = (scope, ast_id.into());
                let mut ctxt = lower::Context {
                    db,
                    src_map: &mut src_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry,
                };

                body.entry_stmts = ast_node.body().map(|stmt| ctxt.collect_stmt(stmt)).collect();
            }
            DefWithBodyId::VarId(id) => {
                let var = id.lookup(db);
                let ast_id = var.ast_id(db);
                let ast_node = var.source(db);
                let curr_scope = (var.scope, ast_id.into());
                let mut ctxt = lower::Context {
                    db,
                    src_map: &mut src_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry,
                };
                let expr = if let Some(expr) = ast_node.default() {
                    ctxt.collect_expr(expr)
                } else {
                    let default_val = match db.var_data(id).ty {
                        Type::Real => Literal::Float(Ieee64::with_float(0.0)),
                        Type::Integer => Literal::Int(0),
                        _ => unreachable!("invalid var type (TODO arrays)"),
                    };
                    ctxt.alloc_expr_desugared(Expr::Literal(default_val))
                };
                let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(expr));

                body.entry_stmts = Box::from([stmt])
            }
            DefWithBodyId::NatureAttrId(attr) => {
                let ast = db.parse(root_file).tree();
                let item_tree = db.item_tree(root_file);

                let NatureAttrLoc { nature, id } = attr.lookup(db);
                let NatureLoc { root_file, id: nature_id } = nature.lookup(db);

                let nature = &item_tree[nature_id];
                let idx = usize::from(nature.attrs.start()) + usize::from(id);
                let attr = &item_tree[ItemTreeId::<NatureAttr>::from(idx)];
                let ast_node = ast_id_map.get(attr.ast_id).to_node(ast.syntax());
                let curr_scope = (ScopeId::root(root_file), attr.ast_id.into());
                let mut ctxt = lower::Context {
                    db,
                    src_map: &mut src_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry,
                };
                let expr = ctxt.collect_expr_opt(ast_node.val());
                let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(expr));

                body.entry_stmts = Box::from([stmt])
            }
            DefWithBodyId::DisciplineAttrId(attr) => {
                let ast = db.parse(root_file).tree();
                let item_tree = db.item_tree(root_file);

                let DisciplineAttrLoc { discipline, id } = attr.lookup(db);
                let DisciplineLoc { root_file, id: discipline_id } = discipline.lookup(db);

                let discipline = &item_tree[discipline_id];
                let idx = usize::from(discipline.attrs.start()) + usize::from(id);
                let attr = &item_tree[ItemTreeId::<DisciplineAttr>::from(idx)];
                let ast_node = ast_id_map.get(attr.ast_id).to_node(ast.syntax());
                let curr_scope = (ScopeId::root(root_file), attr.ast_id.into());

                let mut ctxt = lower::Context {
                    db,
                    src_map: &mut src_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry,
                };
                let expr = ctxt.collect_expr_opt(ast_node.val());
                let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(expr));

                body.entry_stmts = Box::from([stmt])
            }
        }

        (Arc::new(body), Arc::new(src_map))
    }

    pub fn param_body_with_sourcemap_query(
        db: &dyn HirDefDB,
        id: ParamId,
    ) -> (Arc<Body>, Arc<BodySourceMap>, ParamExprs) {
        let mut body = Body::default();
        let mut src_map = BodySourceMap::default();
        let root_file = id.lookup(db).scope.root_file;
        let ast_id_map = db.ast_id_map(root_file);
        let registry = &db.lint_registry();

        let param = id.lookup(db);
        let ast_id = param.ast_id(db);
        let ast_node = param.source(db);
        let mut ctxt = lower::Context {
            db,
            src_map: &mut src_map,
            body: &mut body,
            ast_id_map: &ast_id_map,
            curr_scope: (param.scope, ast_id.into()),
            registry,
        };

        let default = ctxt.collect_expr_opt(ast_node.default());
        let mut entry_stmts = vec![ctxt.alloc_stmt_desugared(Stmt::Expr(default))];

        let bounds = ast_node
            .constraints()
            .filter_map(|constraint| {
                let kind = constraint.kind()?;
                let val = match constraint.val()? {
                    ast::ConstraintValue::Val(val) => {
                        let val = ctxt.collect_expr(val);
                        let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(val));
                        entry_stmts.push(stmt);
                        ConstraintValue::Value(val)
                    }
                    ast::ConstraintValue::Range(range) => {
                        let start = ctxt.collect_expr_opt(range.start());
                        let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(start));
                        entry_stmts.push(stmt);

                        let end = ctxt.collect_expr_opt(range.end());
                        let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(end));
                        entry_stmts.push(stmt);

                        ConstraintValue::Range(Range {
                            start,
                            start_inclusive: range.start_inclusive(),
                            end,
                            end_inclusive: range.end_inclusive(),
                        })
                    }
                };
                Some(ParamConstraint { kind, val })
            })
            .collect();
        body.entry_stmts = Box::from(entry_stmts);

        (Arc::new(body), Arc::new(src_map), ParamExprs { default, bounds })
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct ParamExprs {
    pub default: ExprId,
    pub bounds: Arc<[ParamConstraint]>,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub struct ParamConstraint {
    pub kind: ast::ConstraintKind,
    pub val: ConstraintValue,
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ConstraintValue {
    Value(ExprId),
    Range(Range),
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub struct Range {
    pub start: ExprId,
    pub start_inclusive: bool,
    pub end: ExprId,
    pub end_inclusive: bool,
}
