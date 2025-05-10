use super::*;

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Stmt<'a> {
    Expr(ExprId),
    Block { body: &'a [StmtId] },
    Assignment { lhs: AssignmentLhs, rhs: ExprId },
    Contribute { kind: ContributeKind, lhs: BranchWrite, rhs: ExprId },
    If { cond: ExprId, then_stmt: StmtId, else_stmt: StmtId },
    WhileLoop { cond: ExprId, body: StmtId },
    ForLoop { init: StmtId, cond: ExprId, incr: StmtId, body: StmtId },
    Case { discr: ExprId, case_arms: &'a [Case] }, // TODO lint on unreachable
    EventControl { event: &'a Event, body: StmtId },
}
impl Stmt<'_> {
    #[inline]
    pub fn unwrap_expr(&self) -> ExprId {
        let Stmt::Expr(id) = *self else { unreachable!("Called unwrap_expr on {:?}", self) };
        id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum AssignmentLhs {
    Variable(Variable),
    FunctionReturn(Function),
    FunctionArg(FunctionArg),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ContributeKind {
    Flow,
    Potential,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum Expr<'a> {
    Read(Ref),
    UnaryOp { arg: ExprId, op: UnaryOp },
    BinaryOp { lhs: ExprId, rhs: ExprId, op: BinaryOp },
    Select { cond: ExprId, then_expr: ExprId, else_expr: ExprId },
    Call { fun: ResolvedFun, args: &'a [ExprId] },
    Literal(&'a Literal),
    Array(&'a [ExprId]),
}
impl Expr<'_> {
    pub fn is_literal_zero(&self) -> bool {
        if let Expr::Literal(lit) = self {
            lit.is_zero()
        } else {
            false
        }
    }

    pub fn as_assignment_lhs(&self) -> AssignmentLhs {
        match *self {
            Expr::Read(Ref::Variable(var)) => AssignmentLhs::Variable(var),
            Expr::Read(Ref::FunctionArg(arg)) => AssignmentLhs::FunctionArg(arg),
            Expr::Read(Ref::FunctionReturn(fun)) => AssignmentLhs::FunctionReturn(fun),
            _ => panic!("{self:?} is not a lhs reference"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ref {
    NatureAttr(NatureAttr),
    Variable(Variable),
    Parameter(Parameter),
    ParamSysFun(ParamSysFun),
    FunctionArg(FunctionArg),
    FunctionReturn(Function),
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ResolvedFun {
    User { func: Function, limit: bool },
    BuiltIn(BuiltIn),
}
