use super::*;

use crate::ParamId;

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
