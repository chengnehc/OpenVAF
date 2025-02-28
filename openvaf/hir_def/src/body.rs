use std::sync::Arc;
use stdx::Ieee64;

use ahash::AHashMap as HashMap;
use arena::{Arena, ArenaMap};
use basedb::lints::{Lint, LintSrc};
use basedb::{AttrDiagnostic, LintAttrs};
use syntax::{ast, AstNode, AstPtr};

use crate::db::HirDefDB;
use crate::item_tree::{DisciplineAttr, ItemTreeId, ItemTreeNode, NatureAttr};
use crate::nameres::{DefMapSource, LocalScopeId};
use crate::{
    DefWithBodyId, DisciplineAttrLoc, DisciplineLoc, Expr, ExprId, FunctionLoc, Literal, Lookup,
    ModuleLoc, NatureAttrLoc, NatureLoc, ParamId, ParamLoc, ScopeId, Stmt, StmtId, Type, VarLoc,
};

mod lower;
mod pretty;

/// The body of an item (functions, analog block, etc.)
#[derive(Debug, Eq, PartialEq, Default)]
pub struct Body {
    pub exprs: Arena<Expr>,
    pub stmt_scopes: ArenaMap<Stmt, ScopeId>,
    pub stmts: Arena<Stmt>,
    pub entry_stmts: Box<[StmtId]>,
}

/// The mapping from AST nodes to HIR expression/statement IDs (and back).
/// This is needed to go from e.g. a position in a file to the HIR expression/
/// statement containing it.
///
/// But for type inference etc., we want to operate on a structure that is
/// agnostic to the actual positions of expressions in the file, so that we
/// don't recompute types whenever some whitespace is typed.
#[derive(Default, Debug, Eq, PartialEq)]
pub struct BodySourceMap {
    pub expr_map: HashMap<AstPtr<ast::Expr>, ExprId>,
    pub expr_map_back: ArenaMap<Expr, Option<AstPtr<ast::Expr>>>,
    pub stmt_map: HashMap<AstPtr<ast::Stmt>, StmtId>,
    pub stmt_map_back: ArenaMap<Stmt, Option<AstPtr<ast::Stmt>>>,
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
        let mut source_map = BodySourceMap::default();

        let root_file = def.file(db);
        let ast = db.parse(root_file).tree();
        let item_tree = db.item_tree(root_file);
        let ast_id_map = db.ast_id_map(root_file);
        let registry = db.lint_registry();

        match def {
            DefWithBodyId::ParamId(param) => {
                let (body, sm, _) = db.param_body_with_sourcemap(param);
                return (body, sm);
            }
            DefWithBodyId::ModuleId { initial, module } => {
                let ModuleLoc { scope, id } = module.lookup(db);

                let ast_id = item_tree[id].ast_id();
                let ast_node = ast_id_map.get(ast_id).to_node(ast.syntax());
                let curr_scope = (scope, ast_id.into());

                let mut ctx = lower::Ctx {
                    db,
                    source_map: &mut source_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry: &registry,
                };
                body.entry_stmts = if initial {
                    ast_node.analog_initial_behaviour().map(|stmt| ctx.collect_stmt(stmt)).collect()
                } else {
                    ast_node.analog_behaviour().map(|stmt| ctx.collect_stmt(stmt)).collect()
                };
            }
            DefWithBodyId::FunctionId(fun) => {
                let FunctionLoc { id, .. } = fun.lookup(db);

                let scope = ScopeId {
                    root_file,
                    local_id: LocalScopeId::from(0u32),
                    src: DefMapSource::Function(fun),
                };
                debug_assert_eq!(scope.local_id, db.function_def_map(fun).entry_scope());

                let ast_id = item_tree[id].ast_id();
                let ast_node = ast_id_map.get(ast_id).to_node(ast.syntax());
                let curr_scope = (scope, ast_id.into());

                let mut ctx = lower::Ctx {
                    db,
                    source_map: &mut source_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry: &registry,
                };
                body.entry_stmts = ast_node.body().map(|stmt| ctx.collect_stmt(stmt)).collect();
            }
            DefWithBodyId::VarId(var) => {
                let VarLoc { scope, id } = var.lookup(db);

                let ast_id = item_tree[id].ast_id();
                let ast_node = ast_id_map.get(ast_id).to_node(ast.syntax());

                let curr_scope = (scope, ast_id.into());
                let mut ctx = lower::Ctx {
                    db,
                    source_map: &mut source_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry: &registry,
                };

                let expr = if let Some(expr) = ast_node.default() {
                    ctx.collect_expr(expr)
                } else {
                    let default_val = match db.var_data(var).ty {
                        Type::Real => Literal::Float(Ieee64::with_float(0.0)),
                        Type::Integer => Literal::Int(0),
                        _ => unreachable!("invalid var type (TODO arrays)"),
                    };
                    ctx.alloc_expr_desugared(Expr::Literal(default_val))
                };
                let stmt = ctx.alloc_stmt_desugared(Stmt::Expr(expr));
                body.entry_stmts = vec![stmt].into_boxed_slice();
            }
            DefWithBodyId::NatureAttrId(attr) => {
                let NatureAttrLoc { nature, id } = attr.lookup(db);
                let NatureLoc { root_file, id: discipline_id } = nature.lookup(db);

                let nature = &item_tree[discipline_id];
                let idx = usize::from(nature.attrs.start()) + usize::from(id);
                let attr = &item_tree[ItemTreeId::<NatureAttr>::from(idx)];

                let ast = ast_id_map.get(attr.ast_id).to_node(ast.syntax());
                let curr_scope = (ScopeId::root(root_file), attr.ast_id.into());

                let mut ctx = lower::Ctx {
                    db,
                    source_map: &mut source_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry: &registry,
                };
                let expr = ctx.collect_expr_opt(ast.val());
                let stmt = ctx.alloc_stmt_desugared(Stmt::Expr(expr));
                body.entry_stmts = vec![stmt].into_boxed_slice();
            }
            DefWithBodyId::DisciplineAttrId(attr) => {
                let DisciplineAttrLoc { discipline, id } = attr.lookup(db);
                let DisciplineLoc { root_file, id: discipline_id } = discipline.lookup(db);

                let discipline = &item_tree[discipline_id];
                let idx = usize::from(discipline.attrs.start()) + usize::from(id);
                let attr = &item_tree[ItemTreeId::<DisciplineAttr>::from(idx)];
                let ast = ast_id_map.get(attr.ast_id).to_node(ast.syntax());
                let curr_scope = (ScopeId::root(root_file), attr.ast_id.into());

                let mut ctx = lower::Ctx {
                    db,
                    source_map: &mut source_map,
                    body: &mut body,
                    ast_id_map: &ast_id_map,
                    curr_scope,
                    registry: &registry,
                };
                let expr = ctx.collect_expr_opt(ast.val());
                let stmt = ctx.alloc_stmt_desugared(Stmt::Expr(expr));
                body.entry_stmts = vec![stmt].into_boxed_slice();
            }
        }

        (Arc::new(body), Arc::new(source_map))
    }

    pub fn param_body_with_sourcemap_query(
        db: &dyn HirDefDB,
        id: ParamId,
    ) -> (Arc<Body>, Arc<BodySourceMap>, ParamExprs) {
        let mut body = Body::default();
        let mut source_map = BodySourceMap::default();
        let root_file = id.lookup(db).scope.root_file;

        let tree = db.item_tree(root_file);
        let ast_id_map = db.ast_id_map(root_file);
        let ast = db.parse(root_file).tree();

        let ParamLoc { id: item_tree, scope } = id.lookup(db);
        let ast_id = tree[item_tree].ast_id();
        let ast = ast_id_map.get(ast_id).to_node(ast.syntax());

        let registry = db.lint_registry();
        let mut ctx = lower::Ctx {
            db,
            source_map: &mut source_map,
            body: &mut body,
            ast_id_map: &ast_id_map,
            curr_scope: (scope, ast_id.into()),
            registry: &registry,
        };

        let default = ctx.collect_expr_opt(ast.default());
        let mut entry_stmts = vec![ctx.alloc_stmt_desugared(Stmt::Expr(default))];

        let bounds = ast
            .constraints()
            .filter_map(|constraint| {
                let kind = constraint.kind()?;
                let val = match constraint.val()? {
                    ast::ConstraintValue::Val(val) => {
                        let val = ctx.collect_expr(val);
                        let stmt = ctx.alloc_stmt_desugared(Stmt::Expr(val));
                        entry_stmts.push(stmt);
                        ConstraintValue::Value(val)
                    }
                    ast::ConstraintValue::Range(range) => {
                        let start = ctx.collect_expr_opt(range.start());
                        let stmt = ctx.alloc_stmt_desugared(Stmt::Expr(start));
                        entry_stmts.push(stmt);

                        let end = ctx.collect_expr_opt(range.end());
                        let stmt = ctx.alloc_stmt_desugared(Stmt::Expr(end));
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

        body.entry_stmts = entry_stmts.into_boxed_slice();

        (Arc::new(body), Arc::new(source_map), ParamExprs { default, bounds })
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
