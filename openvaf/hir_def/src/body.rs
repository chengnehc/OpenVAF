use std::sync::Arc;
use stdx::Ieee64;

use ahash::AHashMap as HashMap;
use arena::{Arena, ArenaMap};
use basedb::lints::{Lint, LintSrc};
use basedb::{AttrDiagnostic, LintAttrs};
use syntax::{ast, AstPtr};

use crate::db::HirDefDB;
use crate::item_tree::{DisciplineAttr, ItemTreeId, NatureAttr};
use crate::nameres::{DefMapSource, LocalScopeId};
use crate::{
    DefWithBodyId, DisciplineAttrLoc, DisciplineLoc, Lookup, NatureAttrLoc, NatureLoc, ParamId,
    Scope, Type,
};

mod expr;
mod lower;
mod pretty;

pub use expr::{Case, CaseCond, Event, Expr, ExprId, GlobalEvent, Literal, Stmt, StmtId};

/// The body of an item that contains HIR-level expressions and statements.
#[derive(Debug, Eq, PartialEq, Default)]
pub struct Body {
    pub exprs: Arena<Expr>,
    pub stmts: Arena<Stmt>,
    pub entry_stmts: Box<[StmtId]>,
    pub stmt_scopes: ArenaMap<Stmt, Scope>,
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
    pub fn body_with_srcmap_query(
        db: &dyn HirDefDB,
        def: DefWithBodyId,
    ) -> (Arc<Body>, Arc<BodySourceMap>) {
        let mut body = Body::default();
        let mut src_map = BodySourceMap::default();

        let root_file = def.file(db);
        let ast_id_map = &db.ast_id_map(root_file);

        match def {
            DefWithBodyId::NatureAttrId(attr) => {
                let root = db.parse(root_file).syntax_node();
                let item_tree = db.item_tree(root_file);

                let NatureAttrLoc { nature, id } = attr.lookup(db);
                let NatureLoc { root_file, id: nature_id } = nature.lookup(db);

                let nature = &item_tree[nature_id];
                let idx = usize::from(nature.attrs.start()) + usize::from(id);
                let attr = &item_tree[ItemTreeId::<NatureAttr>::from(idx)];
                let ast_node = ast_id_map.get(attr.ast_id).to_node(&root);
                let curr_scope = (Scope::root(root_file), attr.ast_id.into());
                let mut ctxt = lower::Context {
                    src_map: &mut src_map,
                    body: &mut body,
                    curr_scope,
                    ast_id_map,
                    db,
                };
                let expr = ctxt.collect_expr_opt(ast_node.val());
                let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(expr));
                body.entry_stmts = Box::from([stmt]);
            }
            DefWithBodyId::DisciplineAttrId(attr) => {
                let root = db.parse(root_file).syntax_node();
                let item_tree = db.item_tree(root_file);

                let DisciplineAttrLoc { discipline, id } = attr.lookup(db);
                let DisciplineLoc { root_file, id: discipline_id } = discipline.lookup(db);

                let discipline = &item_tree[discipline_id];
                let idx = usize::from(discipline.attrs.start()) + usize::from(id);
                let attr = &item_tree[ItemTreeId::<DisciplineAttr>::from(idx)];
                let ast_node = ast_id_map.get(attr.ast_id).to_node(&root);
                let curr_scope = (Scope::root(root_file), attr.ast_id.into());
                let mut ctxt = lower::Context {
                    src_map: &mut src_map,
                    body: &mut body,
                    curr_scope,
                    ast_id_map,
                    db,
                };
                let expr = ctxt.collect_expr_opt(ast_node.val());
                let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(expr));
                body.entry_stmts = Box::from([stmt]);
            }
            DefWithBodyId::ModuleId { initial, id } => {
                let module = id.lookup(db);
                let ast_id = module.ast_id(db);
                let ast_node = module.source(db);
                let curr_scope = (module.scope, ast_id.into());
                let mut ctxt = lower::Context {
                    src_map: &mut src_map,
                    body: &mut body,
                    curr_scope,
                    ast_id_map,
                    db,
                };
                body.entry_stmts = if initial {
                    ast_node
                        .analog_initial_behaviors()
                        .map(|stmt| ctxt.collect_stmt(stmt))
                        .collect()
                } else {
                    ast_node.analog_behaviors().map(|stmt| ctxt.collect_stmt(stmt)).collect()
                };
            }
            DefWithBodyId::VarId(id) => {
                let var = id.lookup(db);
                let ast_id = var.ast_id(db);
                let ast_node = var.source(db);
                let curr_scope = (var.scope, ast_id.into());
                let mut ctxt = lower::Context {
                    src_map: &mut src_map,
                    body: &mut body,
                    curr_scope,
                    ast_id_map,
                    db,
                };
                let expr = if let Some(expr) = ast_node.initial() {
                    // the variable is explicitly initialized
                    ctxt.collect_expr(expr)
                } else {
                    // initialize the variable with zero if it is not initialized
                    let default_val = match db.var_data(id).ty {
                        Type::Real => Literal::Float(Ieee64::with_float(0.0)),
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
            DefWithBodyId::ParamId(param) => {
                let (body, sm, _) = db.param_body_with_srcmap(param);
                return (body, sm);
            }
            DefWithBodyId::FunctionId(id) => {
                let scope =
                    Scope::from(root_file, DefMapSource::Function(id), LocalScopeId::from(0u32));
                debug_assert_eq!(scope.local_id, db.function_def_map(id).entry_scope());

                let fun = id.lookup(db);
                let ast_id = fun.ast_id(db);
                let ast_node = fun.source(db);
                let curr_scope = (scope, ast_id.into());
                let mut ctxt = lower::Context {
                    src_map: &mut src_map,
                    body: &mut body,
                    curr_scope,
                    ast_id_map,
                    db,
                };
                body.entry_stmts = ast_node.body().map(|stmt| ctxt.collect_stmt(stmt)).collect();
            }
        }

        (Arc::new(body), Arc::new(src_map))
    }

    pub fn param_body_with_srcmap_query(
        db: &dyn HirDefDB,
        id: ParamId,
    ) -> (Arc<Body>, Arc<BodySourceMap>, ParamExprs) {
        let mut body = Body::default();
        let mut src_map = BodySourceMap::default();

        let param = id.lookup(db);
        let ast_id_map = db.ast_id_map(param.scope.root_file);
        let ast_id = param.ast_id(db);
        let ast_node = param.source(db);
        let mut ctxt = lower::Context {
            body: &mut body,
            src_map: &mut src_map,
            ast_id_map: &ast_id_map,
            curr_scope: (param.scope, ast_id.into()),
            db,
        };

        let default = ctxt.collect_expr_opt(ast_node.default());
        let mut entry_stmts = vec![ctxt.alloc_stmt_desugared(Stmt::Expr(default))];

        let constraints = ast_node
            .constraints()
            .filter_map(|constraint| {
                let kind = constraint.kind()?;
                let val = match constraint.val()? {
                    ast::ConstraintValue::Value(val) => {
                        let val = ctxt.collect_expr(val);
                        let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(val));
                        entry_stmts.push(stmt);
                        ConstraintValue::Value(val)
                    }
                    ast::ConstraintValue::Range(range) => {
                        let lower_bound = ctxt.collect_expr_opt(range.lower_bound());
                        let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(lower_bound));
                        entry_stmts.push(stmt);

                        let upper_bound = ctxt.collect_expr_opt(range.upper_bound());
                        let stmt = ctxt.alloc_stmt_desugared(Stmt::Expr(upper_bound));
                        entry_stmts.push(stmt);

                        ConstraintValue::Range(Range {
                            lower_bound,
                            lower_inclusive: range.lower_inclusive(),
                            upper_bound,
                            upper_inclusive: range.upper_inclusive(),
                        })
                    }
                };
                Some(ParamConstraint { kind, val })
            })
            .collect();
        // entry stmts contain parameter default value and constexprs of contraints
        body.entry_stmts = Box::from(entry_stmts);

        (Arc::new(body), Arc::new(src_map), ParamExprs { default, constraints })
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct ParamExprs {
    pub default: ExprId,
    pub constraints: Arc<[ParamConstraint]>,
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
    pub lower_bound: ExprId,
    pub lower_inclusive: bool,
    pub upper_bound: ExprId,
    pub upper_inclusive: bool,
}
