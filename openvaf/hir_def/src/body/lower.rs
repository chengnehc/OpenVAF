//! Transforms `ast::Expr` and `ast::Stmt` into equivalent `hir_def::expr::Expr`
//! and `hir_def::expr::Stmt` representation.

use std::mem;

use basedb::lints::LintRegistry;
use basedb::{AstIdMap, ErasedAstId, LintAttrs};
use syntax::ast::{self, ArgListOwner, AttrIter, AttrsOwner, FunctionRef};
use syntax::name::AsName;
use syntax::AstPtr;

use crate::db::HirDefDB;
use crate::expr::{CaseCond, Event, GlobalEvent};
use crate::nameres::DefMapSource;
use crate::{BlockLoc, Case, Expr, ExprId, Intern, Literal, Path, ScopeId, Stmt, StmtId};

use super::{Body, BodySourceMap};

pub(super) struct Context<'a> {
    pub(super) db: &'a dyn HirDefDB,
    pub(super) body: &'a mut Body,
    pub(super) source_map: &'a mut BodySourceMap,
    pub(super) ast_id_map: &'a AstIdMap,
    pub(super) curr_scope: (ScopeId, ErasedAstId),
    pub(super) registry: &'a LintRegistry,
}

impl Context<'_> {
    pub fn collect_expr_opt(&mut self, expr: Option<ast::Expr>) -> ExprId {
        match expr {
            Some(expr) => self.collect_expr(expr),
            None => self.missing_expr(),
        }
    }

    pub fn collect_expr(&mut self, expr: ast::Expr) -> ExprId {
        let e = match &expr {
            ast::Expr::PrefixExpr(e) => {
                let expr = self.collect_expr_opt(e.expr());
                if let Some(op) = e.op_kind() {
                    Expr::UnaryOp { expr, op }
                } else {
                    Expr::Missing
                }
            }

            ast::Expr::BinExpr(e) => {
                let lhs = self.collect_expr_opt(e.lhs());
                let rhs = self.collect_expr_opt(e.rhs());
                Expr::BinaryOp { lhs, rhs, op: e.op_kind() }
            }

            ast::Expr::ParenExpr(e) => return self.collect_expr_opt(e.expr()),

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

            ast::Expr::SelectExpr(e) => {
                let cond = self.collect_expr_opt(e.condition());
                let then_val = self.collect_expr_opt(e.then_val());
                let else_val = self.collect_expr_opt(e.else_val());
                Expr::Select { cond, then_val, else_val }
            }

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

            ast::Expr::Literal(lit) => Expr::Literal(Literal::new(lit.kind())),
        };

        self.alloc_expr(e, AstPtr::new(&expr))
    }

    pub fn collect_stmt_opt(&mut self, stmt: Option<ast::Stmt>) -> StmtId {
        match stmt {
            Some(stmt) => self.collect_stmt(stmt),
            None => self.missing_stmt(),
        }
    }

    pub fn collect_stmt(&mut self, stmt: ast::Stmt) -> StmtId {
        let s = match &stmt {
            ast::Stmt::EmptyStmt(_) => Stmt::Empty,
            ast::Stmt::AssignStmt(stmt) => match stmt.assign() {
                Some(a) => Stmt::Assignment {
                    dst: self.collect_expr_opt(a.lval()),
                    val: self.collect_expr_opt(a.rval()),
                    op_kind: a.op().unwrap(),
                },
                None => {
                    // debug!(tree = debug(stmt), src = display(stmt), "Assign Statement without assign?");
                    Stmt::Missing
                }
            },
            ast::Stmt::ExprStmt(stmt) => Stmt::Expr(self.collect_expr_opt(stmt.expr())),
            ast::Stmt::IfStmt(stmt) => {
                let cond = self.collect_expr_opt(stmt.condition());
                let then_branch = self.collect_stmt_opt(stmt.then_branch());
                let else_branch = self.collect_stmt_opt(stmt.else_branch());
                Stmt::If { cond, then_branch, else_branch }
            }
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
            ast::Stmt::CaseStmt(stmt) => self.collect_case_stmt(stmt),
            ast::Stmt::EventStmt(stmt) => return self.collect_event_stmt(stmt),
            ast::Stmt::BlockStmt(stmt) => self.collect_block(stmt),
        };

        self.alloc_stmt(s, AstPtr::new(&stmt), stmt.attrs())
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
                    let vals = case.exprs().map(|e| self.collect_expr(e)).collect();
                    CaseCond::Vals(vals)
                };
                Case { cond, body: self.collect_stmt_opt(case.stmt()) }
            })
            .collect();

        Stmt::Case { discr, case_arms }
    }

    fn collect_event_stmt(&mut self, event_stmt: &ast::EventStmt) -> StmtId {
        let kind = if event_stmt.initial_step_token().is_some() {
            GlobalEvent::InitialStep
        } else if event_stmt.final_step_token().is_some() {
            GlobalEvent::FinalStep
        } else {
            return self.collect_stmt_opt(event_stmt.stmt());
        };

        let phases = event_stmt.sim_phases().map(|lit| lit.unescaped_value()).collect();
        let event = Event::Global { kind, phases };
        let stmt = Stmt::EventControl { event, body: self.collect_stmt_opt(event_stmt.stmt()) };

        self.alloc_stmt(stmt, AstPtr::new(event_stmt).cast().unwrap(), event_stmt.attrs())
    }

    pub fn collect_block(&mut self, block: &ast::BlockStmt) -> Stmt {
        let ast = self.ast_id_map.id_of(block);
        let id = BlockLoc { ast_id: ast, parent: self.curr_scope.0 }.intern(self.db);
        let scope = self.db.block_def_map(id);

        let parent_scope = match scope {
            Some(def_map) => {
                let scope = ScopeId {
                    root_file: self.curr_scope.0.root_file,
                    local_id: def_map.entry_scope(),
                    src: DefMapSource::Block(id),
                };

                mem::replace(&mut self.curr_scope, (scope, ast.into()))
            }
            None => {
                let scope = self.curr_scope.0;
                mem::replace(&mut self.curr_scope, (scope, ast.into()))
            }
        };

        let body = block.body().map(|stmt| self.collect_stmt(stmt)).collect();

        self.curr_scope = parent_scope;
        Stmt::Block { body }
    }

    fn alloc_expr(&mut self, expr: Expr, ptr: AstPtr<ast::Expr>) -> ExprId {
        let id = self.make_expr(expr, Some(ptr.clone()));
        self.source_map.expr_map.insert(ptr, id);
        id
    }

    pub(super) fn alloc_expr_desugared(&mut self, expr: Expr) -> ExprId {
        self.make_expr(expr, None)
    }

    fn missing_expr(&mut self) -> ExprId {
        self.alloc_expr_desugared(Expr::Missing)
    }

    fn make_expr(&mut self, expr: Expr, src: Option<AstPtr<ast::Expr>>) -> ExprId {
        let id = self.body.exprs.push_and_get_key(expr);
        self.source_map.expr_map_back.insert(id, src);
        id
    }

    fn alloc_stmt(&mut self, stmt: Stmt, ptr: AstPtr<ast::Stmt>, attrs: AttrIter) -> StmtId {
        let attrs = LintAttrs::resolve(
            self.registry,
            attrs,
            &mut self.source_map.diagnostics,
            self.curr_scope.1,
        );
        let id = self.make_stmt(stmt, Some(ptr.clone()), attrs);
        self.source_map.stmt_map.insert(ptr, id);

        id
    }

    pub(super) fn alloc_stmt_desugared(&mut self, stmt: Stmt) -> StmtId {
        self.make_stmt(stmt, None, LintAttrs::empty(self.curr_scope.1))
    }

    pub(super) fn missing_stmt(&mut self) -> StmtId {
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
        let id3 = self.source_map.lint_map.push_and_get_key(attrs);
        debug_assert_eq!(id, id2);
        debug_assert_eq!(id2, id3);
        self.source_map.stmt_map_back.insert(id, src);
        id
    }
}
