//! Lower AST expressions and statements into HIR representation and collect them into `Body`.

use std::mem;

use basedb::{AstIdMap, ErasedAstId};
use syntax::ast::{ArgListOwner, AttrsOwner, FunctionRef};
use syntax::name::AsName;

use super::*;

use crate::nameres::DefMapSource;
use crate::{BlockLoc, Intern, Path};

pub(super) struct Context<'a> {
    // mutates
    pub(super) body: &'a mut Body,
    pub(super) src_map: &'a mut BodySourceMap,
    // reads
    pub(super) db: &'a dyn HirDefDB,
    pub(super) ast_id_map: &'a AstIdMap,
    // states
    pub(super) curr_scope: (ScopeId, ErasedAstId),
}

impl Context<'_> {
    /* Expressions */

    pub(super) fn collect_expr_opt(&mut self, expr: Option<ast::Expr>) -> ExprId {
        match expr {
            Some(expr) => self.collect_expr(expr),
            None => self.missing_expr(),
        }
    }

    pub(super) fn collect_expr(&mut self, expr: ast::Expr) -> ExprId {
        let e = match &expr {
            ast::Expr::Literal(lit) => Expr::Literal(Literal::new(lit.kind())),
            ast::Expr::PathExpr(path) => {
                if let Some(path) = path.path().and_then(Path::resolve) {
                    Expr::Path { path, port: false }
                } else {
                    return self.missing_expr();
                }
            }
            ast::Expr::PortFlow(port_flow) => {
                if let Some(path) = port_flow.port().and_then(Path::resolve) {
                    Expr::Path { path, port: true }
                } else {
                    return self.missing_expr();
                }
            }
            ast::Expr::ParenExpr(e) => return self.collect_expr_opt(e.expr()),
            ast::Expr::PrefixExpr(e) => {
                let arg = self.collect_expr_opt(e.expr());
                if let Some(op) = e.op_kind() {
                    Expr::UnaryOp { arg, op }
                } else {
                    Expr::Missing
                }
            }
            ast::Expr::BinExpr(e) => {
                let lhs = self.collect_expr_opt(e.lhs());
                let rhs = self.collect_expr_opt(e.rhs());
                Expr::BinaryOp { lhs, rhs, op: e.op_kind() }
            }
            ast::Expr::SelectExpr(e) => {
                let cond = self.collect_expr_opt(e.condition());
                let then_expr = self.collect_expr_opt(e.then_val());
                let else_expr = self.collect_expr_opt(e.else_val());
                Expr::Select { cond, then_expr, else_expr }
            }
            ast::Expr::ArrayExpr(e) => {
                let vals = e.exprs().map(|expr| self.collect_expr(expr)).collect();
                Expr::Array(vals)
            }
            ast::Expr::Call(call) => {
                let fun = call.function_ref().and_then(|fun| match fun {
                    FunctionRef::Path(path) => Path::resolve(path),
                    FunctionRef::SysFun(fun) => Some(Path::from_ident(fun.as_name())),
                });
                let args = if let Some(args) = call.arg_list().map(|list| list.args()) {
                    args.map(|arg| self.collect_expr(arg)).collect()
                } else {
                    vec![]
                };
                Expr::Call { fun, args }
            }
        };

        self.alloc_expr(e, AstPtr::new(&expr))
    }

    /// By 'desugared', it means that the expression has no corresponding node in the AST.
    #[inline]
    pub(super) fn alloc_expr_desugared(&mut self, expr: Expr) -> ExprId {
        self.make_expr(expr, None)
    }

    #[inline]
    fn missing_expr(&mut self) -> ExprId {
        self.alloc_expr_desugared(Expr::Missing)
    }

    #[inline]
    fn alloc_expr(&mut self, expr: Expr, src: AstPtr<ast::Expr>) -> ExprId {
        self.make_expr(expr, Some(src))
        // let id = self.make_expr(expr, Some(src.clone()));
        // self.src_map.expr_map.insert(src, id);
        // id
    }

    #[inline]
    fn make_expr(&mut self, expr: Expr, src: Option<AstPtr<ast::Expr>>) -> ExprId {
        let id = self.body.exprs.push_and_get_key(expr);
        self.src_map.expr_map.insert(id, src);
        id
    }

    /* Statements */

    pub(super) fn collect_stmt_opt(&mut self, stmt: Option<ast::Stmt>) -> StmtId {
        match stmt {
            Some(stmt) => self.collect_stmt(stmt),
            None => self.missing_stmt(),
        }
    }

    pub(super) fn collect_stmt(&mut self, stmt: ast::Stmt) -> StmtId {
        let s = match &stmt {
            ast::Stmt::EmptyStmt(_) => Stmt::Empty,
            ast::Stmt::ExprStmt(stmt) => Stmt::Expr(self.collect_expr_opt(stmt.expr())),
            ast::Stmt::BlockStmt(stmt) => self.collect_block_stmt(stmt),
            ast::Stmt::AssignStmt(stmt) => match stmt.assign() {
                Some(a) => Stmt::Assignment {
                    dst: self.collect_expr_opt(a.lval()),
                    val: self.collect_expr_opt(a.rval()),
                    op_kind: a.op_kind().unwrap(),
                },
                None => Stmt::Missing,
            },
            ast::Stmt::WhileStmt(stmt) => {
                let cond = self.collect_expr_opt(stmt.condition());
                let body = self.collect_stmt_opt(stmt.body());
                Stmt::WhileLoop { cond, body }
            }
            ast::Stmt::ForStmt(stmt) => {
                let cond = self.collect_expr_opt(stmt.condition());
                let init = self.collect_stmt_opt(stmt.init());
                let incr = self.collect_stmt_opt(stmt.incr());
                let body = self.collect_stmt_opt(stmt.for_body());
                Stmt::ForLoop { init, cond, incr, body }
            }
            ast::Stmt::IfStmt(stmt) => {
                let cond = self.collect_expr_opt(stmt.condition());
                let then_branch = self.collect_stmt_opt(stmt.then_branch());
                let else_branch = self.collect_stmt_opt(stmt.else_branch());
                Stmt::If { cond, then_branch, else_branch }
            }
            ast::Stmt::CaseStmt(stmt) => self.collect_case_stmt(stmt),
            ast::Stmt::EventStmt(stmt) => self.collect_event_stmt(stmt),
        };

        self.alloc_stmt(s, AstPtr::new(&stmt), stmt.attrs())
    }

    fn collect_block_stmt(&mut self, block: &ast::BlockStmt) -> Stmt {
        let ast_id = self.ast_id_map.id_of(block);
        let (curr, _) = self.curr_scope;
        let mut next = curr;
        if block.block_scope().is_some() {
            // Fixed(JW): no need to intern unnamed blocks, intern scoped blocks only.
            let id = BlockLoc { parent: curr, ast_id }.intern(self.db);
            if let Some(def_map) = self.db.block_def_map(id) {
                next =
                    ScopeId::from(curr.root_file, DefMapSource::Block(id), def_map.entry_scope());
            }
        }
        // JW: Be careful of the order: we enter the block's scope first, then we
        // collect body statements, after that we switch back to parent scope.
        let next_scope = (next, ast_id.into());
        let parent_scope = mem::replace(&mut self.curr_scope, next_scope);
        let body = block.body().map(|stmt| self.collect_stmt(stmt)).collect();
        self.curr_scope = parent_scope;

        Stmt::Block { body }
    }

    fn collect_case_stmt(&mut self, case_stmt: &ast::CaseStmt) -> Stmt {
        let discr = self.collect_expr_opt(case_stmt.discriminant());
        let case_arms = case_stmt
            .cases()
            .map(|case| {
                let cond = if case.default_token().is_some() {
                    debug_assert_eq!(case.exprs().next(), None);
                    CaseCond::Default
                } else {
                    let exprs = case.exprs().map(|e| self.collect_expr(e)).collect();
                    CaseCond::Exprs(exprs)
                };
                Case { cond, body: self.collect_stmt_opt(case.stmt()) }
            })
            .collect();

        Stmt::Case { discr, case_arms }
    }

    fn collect_event_stmt(&mut self, event_stmt: &ast::EventStmt) -> Stmt {
        let kind = if event_stmt.initial_step_token().is_some() {
            GlobalEvent::InitialStep
        } else if event_stmt.final_step_token().is_some() {
            GlobalEvent::FinalStep
        } else {
            unreachable!() //return self.collect_stmt_opt(event_stmt.stmt());
        };
        let phases = event_stmt.sim_phases().map(|lit| lit.unescaped_value()).collect();
        let event = Event::Global { kind, phases };
        let body = self.collect_stmt_opt(event_stmt.stmt());

        Stmt::EventControl { event, body }
    }

    fn alloc_stmt(
        &mut self,
        stmt: Stmt,
        src: AstPtr<ast::Stmt>,
        attrs: impl Iterator<Item = ast::Attr>,
    ) -> StmtId {
        let registry = &self.db.lint_registry();
        let attrs =
            LintAttrs::resolve(registry, attrs, &mut self.src_map.diagnostics, self.curr_scope.1);
        self.make_stmt(stmt, Some(src.clone()), attrs)
        // let id = self.make_stmt(stmt, Some(src.clone()), attrs);
        // self.src_map.stmt_map.insert(src, id);
        // id
    }

    /// By 'desugared', it means that the statement has no corresponding node in the AST.
    #[inline]
    pub(super) fn alloc_stmt_desugared(&mut self, stmt: Stmt) -> StmtId {
        self.make_stmt(stmt, None, LintAttrs::empty(self.curr_scope.1))
    }

    #[inline]
    fn missing_stmt(&mut self) -> StmtId {
        self.alloc_stmt_desugared(Stmt::Missing)
    }

    fn make_stmt(
        &mut self,
        stmt: Stmt,
        src: Option<AstPtr<ast::Stmt>>,
        attrs: LintAttrs,
    ) -> StmtId {
        let id = self.body.stmts.push_and_get_key(stmt);
        let id2 = self.body.stmt_scopes.push_and_get_key(self.curr_scope.0);
        let id3 = self.src_map.lint_map.push_and_get_key(attrs);
        debug_assert_eq!(id, id2);
        debug_assert_eq!(id2, id3);
        self.src_map.stmt_map.insert(id, src);
        id
    }
}
