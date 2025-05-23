use std::sync::Arc;

use super::{ast, lower, Body, BodySourceMap, Expr, ExprId, Literal, Stmt};

use crate::item_tree::{DisciplineAttr, ItemTreeId, NatureAttr};
use crate::nameres::{DefMapSource, ItemWithBodyId, LocalScopeId};
use crate::{DisciplineAttrLoc, DisciplineLoc, NatureAttrLoc, NatureLoc, ParamId, ScopeId};
use crate::{HirDefDB, Lookup, Type};

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

impl Body {
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
            db,
            ast_id_map: &ast_id_map,
            curr_scope: (param.scope, ast_id.into()),
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
        // entry stmts contain constexprs of parameter default values and contraints
        body.entry_stmts = Box::from(entry_stmts);

        (Arc::new(body), Arc::new(src_map), ParamExprs { default, constraints })
    }
}
