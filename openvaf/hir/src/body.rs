use std::sync::Arc;

use hir_def::nameres::ItemWithBodyId;
use hir_ty::inference;
use hir_ty::types::{Signature, Ty};

pub use hir_def::body::{Case, Event, ExprId, Literal, StmtId};
pub use hir_def::{BuiltIn, ParamSysFun, Type};
pub use syntax::ast::{BinaryOp, UnaryOp};

use crate::{Branch, BranchWrite, Function, FunctionArg, NatureAttr, Node, Parameter, Variable};
use crate::{CompilationDB, HirDB, HirDefDB};

mod expr;

pub use expr::{AssignmentLhs, ContributeKind, Expr, Ref, ResolvedFun, Stmt};

#[derive(Debug, Clone)]
pub struct Body {
    body: Arc<hir_def::body::Body>,
    infere: Arc<inference::Inference>,
}
impl Body {
    pub(crate) fn new(id: ItemWithBodyId, db: &CompilationDB) -> Body {
        Body { body: db.body(id), infere: db.inference_result(id) }
    }

    pub fn borrow(&self) -> BodyRef<'_> {
        BodyRef { body: &self.body, infere: &self.infere }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BodyRef<'a> {
    body: &'a hir_def::body::Body,
    infere: &'a inference::Inference,
}

/// Statements
impl<'a> BodyRef<'a> {
    pub fn entry_stmts(&self) -> &'a [StmtId] {
        &self.body.entry_stmts
    }

    pub fn get_stmt(&self, stmt: StmtId) -> Option<Stmt<'a>> {
        match self.body.stmts[stmt] {
            hir_def::Stmt::Missing | hir_def::Stmt::Empty => None,
            hir_def::Stmt::Expr(e) => Some(Stmt::Expr(e)),
            hir_def::Stmt::Block { ref body } => Some(Stmt::Block { body }),
            hir_def::Stmt::Assignment { val, .. } => {
                let stmt = match self.infere.assignment_destination[&stmt] {
                    inference::AssignDst::Var(id) => {
                        Stmt::Assignment { lhs: AssignmentLhs::Variable(Variable { id }), rhs: val }
                    }
                    inference::AssignDst::FunVar { fun, arg: None } => Stmt::Assignment {
                        lhs: AssignmentLhs::FunctionReturn(Function { id: fun }),
                        rhs: val,
                    },
                    inference::AssignDst::FunVar { fun, arg: Some(arg) } => Stmt::Assignment {
                        lhs: AssignmentLhs::FunctionArg(FunctionArg { fun, arg }),
                        rhs: val,
                    },
                    inference::AssignDst::Flow(branch) => Stmt::Contribute {
                        kind: ContributeKind::Flow,
                        lhs: branch.into(),
                        rhs: val,
                    },
                    inference::AssignDst::Potential(branch) => Stmt::Contribute {
                        kind: ContributeKind::Potential,
                        lhs: branch.into(),
                        rhs: val,
                    },
                };
                Some(stmt)
            }
            hir_def::Stmt::ForLoop { init, cond, incr, body } => {
                Some(Stmt::ForLoop { init, cond, incr, body })
            }
            hir_def::Stmt::WhileLoop { cond, body } => Some(Stmt::WhileLoop { cond, body }),
            hir_def::Stmt::If { cond, then_branch, else_branch } => {
                Some(Stmt::If { cond, then_stmt: then_branch, else_stmt: else_branch })
            }
            hir_def::Stmt::Case { discr, ref case_arms } => Some(Stmt::Case { discr, case_arms }),
            hir_def::Stmt::EventControl { ref event, body } => {
                Some(Stmt::EventControl { event, body })
            }
        }
    }

    pub fn get_nth_entry_expr(&self, n: usize) -> ExprId {
        let Stmt::Expr(id) = self.get_stmt(self.entry_stmts()[n]).unwrap() else {
            unreachable!("The entry stmt is not a expression.")
        };
        id
    }
}

/// Expression
impl<'a> BodyRef<'a> {
    /* Body-related */

    pub fn get_expr(&self, expr: ExprId) -> Expr<'a> {
        match self.body.exprs[expr] {
            hir_def::Expr::Path { .. } => Expr::Read(self.resolve_path(expr)),
            hir_def::Expr::UnaryOp { arg, op } => Expr::UnaryOp { arg, op },
            hir_def::Expr::BinaryOp { lhs, rhs, op: Some(op) } => Expr::BinaryOp { lhs, rhs, op },
            hir_def::Expr::Select { cond, then_expr, else_expr } => {
                Expr::Select { cond, then_expr, else_expr }
            }
            hir_def::Expr::Call { ref args, .. } => {
                let fun = match self.infere.resolved_calls[&expr] {
                    inference::ResolvedFun::User { func, limit } => {
                        ResolvedFun::User { func: Function { id: func }, limit }
                    }
                    inference::ResolvedFun::BuiltIn(builtin) => ResolvedFun::BuiltIn(builtin),
                    // this is a special case, the VAMS standard allows these parameters
                    // to be called like functions (but its the same as direct access)
                    // we hide that detail from downstream users here
                    inference::ResolvedFun::Param(param) => {
                        return Expr::Read(Ref::ParamSysFun(param));
                    }
                    inference::ResolvedFun::InvalidNatureAccess(_) => {
                        panic!("invalid HIR: invalid nature access {:?}", self.body.exprs[expr])
                    }
                };
                Expr::Call { fun, args }
            }
            hir_def::Expr::Literal(ref literal) => Expr::Literal(literal),
            hir_def::Expr::Array(ref args) => Expr::Array(args),
            _ => panic!("invalid HIR: {:?}", self.body.exprs[expr]),
        }
    }

    fn resolve_path(&self, expr: ExprId) -> Ref {
        match self.infere.expr_types[expr] {
            Ty::NatureAttr(_, id) => Ref::NatureAttr(NatureAttr { id }),
            Ty::Var(_, id) => Ref::Variable(Variable { id }),
            Ty::Param(_, id) => Ref::Parameter(Parameter { id }),
            Ty::FunctionVar { fun, arg: Some(arg), .. } => {
                Ref::FunctionArg(FunctionArg { fun, arg })
            }
            Ty::FunctionVar { fun, .. } => Ref::FunctionReturn(Function { id: fun }),

            ref it => {
                if let Some(&inference::ResolvedFun::Param(param)) =
                    self.infere.resolved_calls.get(&expr)
                {
                    Ref::ParamSysFun(param)
                } else {
                    panic!("invalid HIR: path {:?} was not resolved {it:?}", self.body.exprs[expr])
                }
            }
        }
    }

    pub fn as_literal(&self, expr: ExprId) -> Option<&'a Literal> {
        match &self.body.exprs[expr] {
            hir_def::Expr::Literal(lit) => Some(lit),
            _ => None,
        }
    }

    pub fn as_signed_int_literal(&self, expr: ExprId) -> Option<i32> {
        match &self.body.exprs[expr] {
            // Int literal
            hir_def::Expr::Literal(Literal::Int(ii)) => Some(*ii),
            // Int literal with `-` prefix
            hir_def::Expr::UnaryOp { arg, op: UnaryOp::Neg } => {
                self.as_int_literal(*arg).map(|ii| -ii)
            }
            _ => None,
        }
    }

    fn as_int_literal(&self, expr: ExprId) -> Option<i32> {
        match &self.body.exprs[expr] {
            hir_def::Expr::Literal(Literal::Int(ii)) => Some(*ii),
            _ => None,
        }
    }

    /* Inference-related */

    pub fn expr_type(&self, expr: ExprId) -> Type {
        self.infere.expr_types[expr].to_value().unwrap()
    }

    pub fn get_call_signature(&self, expr: ExprId) -> Signature {
        self.infere.resolved_signatures.get(&expr).copied().unwrap_or(Signature(u32::MAX))
    }

    /// Returns whether the result of an expression needs to be cast to a
    /// different type before use.
    pub fn need_type_cast(&self, expr: ExprId) -> Option<(Type, &'a Type)> {
        let dst = self.infere.casts.get(&expr)?;
        let src = self.expr_type(expr);
        debug_assert_ne!(&src, dst, "cast types must be different");
        Some((src, dst))
    }

    pub fn into_node(&self, expr: ExprId) -> Node {
        let id = self.infere.expr_types[expr].unwrap_node();
        Node { id }
    }

    pub fn into_branch(&self, expr: ExprId) -> Branch {
        let id = self.infere.expr_types[expr].unwrap_branch();
        Branch { id }
    }

    pub fn into_port_flow(&self, expr: ExprId) -> Node {
        let id = self.infere.expr_types[expr].unwrap_port_flow();
        Node { id }
    }

    pub fn into_param(&self, expr: ExprId) -> Parameter {
        let id = self.infere.expr_types[expr].unwrap_param();
        Parameter { id }
    }
}
