use std::ops::Index;

use arena::{Arena, ArenaMap};
use basedb::lints::{Lint, LintAttrDiagnostic, LintAttrs, LintSrc};
use syntax::{ast, AstPtr};

use crate::{nameres::DefMapSource, HirDefDB, Lookup, ScopeId};

mod expr;
mod lower;
mod pretty;
mod query;

pub use expr::*;
pub use query::{ConstraintValue, ParamConstraint, ParamExprs};

/// The body of an item containing HIR-level expressions and statements.
#[derive(Debug, Eq, PartialEq, Default)]
pub struct Body {
    exprs: Arena<Expr>,
    stmts: Arena<Stmt>,
    entry_stmts: Box<[StmtId]>,
    stmt_scopes: ArenaMap<Stmt, ScopeId>,
}

impl Index<ExprId> for Body {
    type Output = Expr;
    fn index(&self, index: ExprId) -> &Self::Output {
        &self.exprs[index]
    }
}

impl Index<StmtId> for Body {
    type Output = Stmt;
    fn index(&self, index: StmtId) -> &Self::Output {
        &self.stmts[index]
    }
}

impl Body {
    pub fn entry_stmts(&self) -> &[StmtId] {
        &self.entry_stmts
    }

    pub fn get_scope_of(&self, stmt: StmtId) -> ScopeId {
        self.stmt_scopes[stmt]
    }

    pub fn capacity(&self) -> (usize, usize) {
        (self.exprs.len(), self.stmts.len())
    }
}

/// The mapping from AST node positions to HIR expression/statement IDs (and in reverse)
#[derive(Default, Debug, Eq, PartialEq)]
pub struct BodySourceMap {
    // JW: commented out as these two maps are not used anywhere
    // pub expr_map: HashMap<AstPtr<ast::Expr>, ExprId>,
    // pub stmt_map: HashMap<AstPtr<ast::Stmt>, StmtId>,
    expr_map: ArenaMap<Expr, Option<AstPtr<ast::Expr>>>,
    stmt_map: ArenaMap<Stmt, Option<AstPtr<ast::Stmt>>>,
    lint_map: ArenaMap<Stmt, LintAttrs>,
    // diagnostics of stmt attributes accumulated during lowering
    pub diagnostics: Vec<LintAttrDiagnostic>,
}

impl Index<ExprId> for BodySourceMap {
    type Output = Option<AstPtr<ast::Expr>>;
    fn index(&self, index: ExprId) -> &Self::Output {
        &self.expr_map[index]
    }
}

impl Index<StmtId> for BodySourceMap {
    type Output = Option<AstPtr<ast::Stmt>>;
    fn index(&self, index: StmtId) -> &Self::Output {
        &self.stmt_map[index]
    }
}

impl BodySourceMap {
    pub fn lint_src(&self, stmt: StmtId, lint: Lint) -> LintSrc {
        self.lint_map[stmt].lint_src(lint)
    }
}
