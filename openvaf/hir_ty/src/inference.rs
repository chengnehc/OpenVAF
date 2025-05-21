//! Type inference, i.e. the process of walking through the code and determining
//! the type of each expression.

use std::borrow::Cow;
use std::iter::zip;
use std::mem;
use std::sync::Arc;

use ahash::AHashMap;
use arena::ArenaMap;
use hir_def::{
    body::{Body, CaseCond, Expr, ExprId, Literal, Stmt, StmtId},
    nameres::{ItemWithBodyId, PathResolveError, ResolvedPath, ScopeItem, ScopeItemKind},
    BranchId, BuiltIn, FunctionArgLoc, FunctionId, LocalFunctionArgId, Lookup, NatureAccess,
    NatureId, NodeId, ParamSysFun, Path, Type, VarId,
};
use syntax::{
    ast::{self, AssignOp, BinaryOp, UnaryOp},
    TextRange, TextSize,
};
use typed_index_collections::{TiSlice, TiVec};

use crate::builtin::{
    BuiltinInfo, DDX_FLOW, DDX_POT, DDX_POT_DIFF, DDX_TEMP, LIMIT_BUILTIN_FUNCTION,
    LIMIT_USER_FUNCTION, NATURE_ACCESS_BRANCH, NATURE_ACCESS_NODES, NATURE_ACCESS_NODE_GND,
    NATURE_ACCESS_PORT_FLOW,
};
use crate::db::{Alias, HirTyDB};
use crate::lower::{BranchTy, DisciplineAccess};
use crate::types::{Signature, SignatureData, Ty, TyRequirement};

mod diagnostics;
mod error;
mod fmt_parser;

pub use diagnostics::InferDiagnosticWrapped;
pub use error::{InferDiagnostic, SignatureMismatch, TypeMismatch};
use fmt_parser::parse_real_fmt_spec;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Inference {
    pub expr_types: ArenaMap<Expr, Ty>,
    pub resolved_calls: AHashMap<ExprId, ResolvedFun>,
    pub resolved_signatures: AHashMap<ExprId, Signature>,
    pub assignment_destination: AHashMap<StmtId, AssignDst>,
    pub casts: AHashMap<ExprId, Type>,
    pub diagnostics: Vec<InferDiagnostic>,
}

impl Inference {
    pub fn infere_body_query(db: &dyn HirTyDB, id: ItemWithBodyId) -> Arc<Inference> {
        let body = db.body(id);
        let result = Inference {
            expr_types: ArenaMap::from(vec![Ty::Val(Type::Err); body.exprs.len()]),
            ..Default::default()
        };
        let mut ctxt = Context { result, body: &body, db, expr_stmt_ty: None };
        // Parameter and variable bodies only contain expressions, whose entry stmts
        // are expr stmts which shall be type checked properly.
        ctxt.expr_stmt_ty = match id {
            ItemWithBodyId::ParamId(param) => match &db.param_data(param).ty {
                Some(ty) => Some(ty.clone()),
                // If the type of a parameter is omitted, it shall be inferred through
                // the parameter's default value. Refer to [LRM 3.4.1]
                None => {
                    let stmt = body.entry_stmts[0];
                    let expr = db.param_exprs(param).default;
                    ctxt.infere_expr(stmt, expr).and_then(|ty| ty.to_value())
                }
            },
            // the type of a variable can not be omitted
            ItemWithBodyId::VarId(var) => Some(db.var_data(var).ty.clone()),
            _ => None,
        };

        for stmt in &body.entry_stmts {
            ctxt.infere_stmt(*stmt);
        }

        Arc::new(ctxt.result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum ResolvedFun {
    User { func: FunctionId, limit: bool },
    BuiltIn(BuiltIn),
    Param(ParamSysFun),
    InvalidNatureAccess(NatureId),
}

#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum AssignDst {
    Var(VarId),
    FunVar { fun: FunctionId, arg: Option<LocalFunctionArgId> },
    Flow(BranchWrite),
    Potential(BranchWrite),
}

#[derive(Debug, Clone, PartialEq, Eq, Copy, Hash)]
pub enum BranchWrite {
    Named(BranchId),
    Unnamed { hi: NodeId, lo: Option<NodeId> },
}

struct Context<'a> {
    result: Inference,
    body: &'a Body,
    db: &'a dyn HirTyDB,
    /// A `Body` that only contains expressions have expr stmts as entry_stmts,
    /// which shall be type checked properly. For behavioural (analog body and
    /// function) and untyped (nature attr) bodies, this is simply None.
    expr_stmt_ty: Option<Type>,
}

impl Context<'_> {
    pub fn infere_stmt(&mut self, stmt: StmtId) {
        match self.body.stmts[stmt] {
            Stmt::Expr(expr) => {
                // TODO lint for side effect free expressions
                self.infere_assignment(stmt, expr, self.expr_stmt_ty.clone());
            }
            Stmt::Assignment { dst, val, op_kind } => {
                let dst_ty = self.infere_assignment_dst(stmt, dst, op_kind);
                self.infere_assignment(stmt, val, dst_ty);
            }
            Stmt::ForLoop { cond, .. } | Stmt::WhileLoop { cond, .. } | Stmt::If { cond, .. } => {
                self.infere_cond(stmt, cond)
            }
            Stmt::Case { discr, ref case_arms } => {
                if let Some(ty) = self.infere_expr(stmt, discr) {
                    let req = ty.to_value().map_or(TyRequirement::AnyVal, TyRequirement::Val);
                    for case in case_arms {
                        let CaseCond::Exprs(exprs) = &case.cond else { continue };
                        for &e in exprs {
                            if let Some(ty) = self.infere_expr(stmt, e) {
                                self.expect::<false>(e, None, ty, Cow::Owned(vec![req.clone()]));
                            }
                        }
                    }
                }
            }
            _ => (),
        };
        self.body.stmts[stmt].walk_child_stmts(|stmt| self.infere_stmt(stmt));
    }

    fn infere_assignment(&mut self, stmt: StmtId, expr: ExprId, dst_ty: Option<Type>) {
        let Some(ty) = self.infere_expr(stmt, expr) else { return };
        if let Some(rhs_ty) = ty.to_value() {
            if let Some(dst_ty) = dst_ty {
                if rhs_ty.is_assignable_to(&dst_ty) {
                    if dst_ty != rhs_ty {
                        self.result.casts.insert(expr, dst_ty);
                    }
                } else {
                    self.result.diagnostics.push(
                        TypeMismatch {
                            expected: Cow::Owned(vec![TyRequirement::Val(dst_ty)]),
                            found_ty: ty,
                            expr,
                        }
                        .into(),
                    );
                }
            }
        } else {
            let expected = dst_ty.map_or(TyRequirement::AnyVal, TyRequirement::Val);
            self.result.diagnostics.push(
                TypeMismatch { expected: Cow::Owned(vec![expected]), found_ty: ty, expr }.into(),
            );
        }
    }

    fn infere_assignment_dst(
        &mut self,
        stmt: StmtId,
        expr: ExprId,
        op_kind: ast::AssignOp,
    ) -> Option<Type> {
        let (dst, ty) = match self.infere_expr(stmt, expr)? {
            Ty::Var(ty, var) => (AssignDst::Var(var), ty),
            Ty::FunctionVar { ty, fun, arg } => (AssignDst::FunVar { fun, arg }, ty),
            Ty::Val(Type::Real)
                if matches!(
                    self.result.resolved_calls.get(&expr),
                    Some(ResolvedFun::BuiltIn(BuiltIn::potential | BuiltIn::flow))
                ) =>
            {
                // deal with contribution LHS
                let mut args = Vec::new();
                self.body.exprs[expr].walk_child_exprs(|e| args.push(&self.result.expr_types[e]));
                let kind = match *self.result.resolved_signatures.get(&expr)? {
                    NATURE_ACCESS_BRANCH => BranchWrite::Named(args[0].unwrap_branch()),
                    NATURE_ACCESS_NODES => BranchWrite::Unnamed {
                        hi: args[0].unwrap_node(),
                        lo: Some(args[1].unwrap_node()),
                    },
                    NATURE_ACCESS_NODE_GND => {
                        BranchWrite::Unnamed { hi: args[0].unwrap_node(), lo: None }
                    }
                    NATURE_ACCESS_PORT_FLOW => {
                        // The port access function shall not be used on the left side
                        // of a contribution operator <+.
                        self.result.diagnostics.push(InferDiagnostic::InvalidAssignDst {
                            expr,
                            op_kind,
                            maybe_different_op: None,
                        });
                        return None;
                    }
                    _ => unreachable!(),
                };
                let dst = match self.result.resolved_calls[&expr] {
                    ResolvedFun::BuiltIn(BuiltIn::potential) => AssignDst::Potential(kind),
                    ResolvedFun::BuiltIn(BuiltIn::flow) => AssignDst::Flow(kind),
                    _ => unreachable!(),
                };

                (dst, Type::Real)
            }
            _ => {
                self.result.diagnostics.push(InferDiagnostic::InvalidAssignDst {
                    expr,
                    op_kind,
                    maybe_different_op: None,
                });
                return None;
            }
        };

        // check that the correct operator is used
        match (&dst, op_kind) {
            (AssignDst::Var(_) | AssignDst::FunVar { .. }, ast::AssignOp::Contribute) => {
                self.result.diagnostics.push(InferDiagnostic::InvalidAssignDst {
                    expr,
                    op_kind,
                    maybe_different_op: Some(ast::AssignOp::Assign),
                });
            }
            (AssignDst::Flow(_) | AssignDst::Potential(_), ast::AssignOp::Assign) => {
                self.result.diagnostics.push(InferDiagnostic::InvalidAssignDst {
                    expr,
                    op_kind,
                    maybe_different_op: Some(ast::AssignOp::Contribute),
                });
            }
            _ => {
                self.result.assignment_destination.insert(stmt, dst);
            }
        }

        Some(ty)
    }

    fn infere_expr(&mut self, stmt: StmtId, expr: ExprId) -> Option<Ty> {
        let ty = match self.body.exprs[expr] {
            Expr::Missing => return None,
            Expr::Path { ref path, port: true } => {
                let port = self.resolve_item_path(stmt, expr, path)?;
                Ty::PortFlow(port)
            }
            Expr::Path { ref path, port: false } => match self.resolve_path(stmt, expr, path)? {
                ScopeItem::NatureId(nature) => Ty::Nature(nature),
                ScopeItem::NatureAttrId(attr) => {
                    Ty::NatureAttr(self.db.nature_attr_ty(attr)?, attr)
                }
                ScopeItem::DisciplineId(discipline) => Ty::Discipline(discipline),
                ScopeItem::ModuleId(_) | ScopeItem::BlockId(_) => Ty::Scope,
                ScopeItem::NodeId(node) => Ty::Node(node),
                ScopeItem::BranchId(branch) => Ty::Branch(branch),
                ScopeItem::VarId(var) => Ty::Var(self.db.var_data(var).ty.clone(), var),
                ScopeItem::ParamId(param) => Ty::Param(self.db.param_ty(param), param),
                ScopeItem::AliasParamId(param) => match self.db.resolve_alias(param)? {
                    Alias::Cycle => return None,
                    Alias::Param(param) => Ty::Param(self.db.param_ty(param), param),
                    Alias::ParamSysFun(param) => {
                        self.result.resolved_calls.insert(expr, ResolvedFun::Param(param));
                        Ty::Val(Type::Real)
                    }
                },
                ScopeItem::ParamSysFun(_) => Ty::Val(Type::Real),
                ScopeItem::BuiltIn(_) | ScopeItem::NatureAccess(_) => Ty::BuiltInFunction,
                ScopeItem::FunctionId(fun) => Ty::UserFunction(fun),
                ScopeItem::FunctionArgId(arg) => {
                    let FunctionArgLoc { fun, id } = arg.lookup(self.db.upcast());
                    let ty = self.db.function_data(fun).args[id].ty.clone();
                    Ty::FunctionVar { ty, fun, arg: Some(id) }
                }
                ScopeItem::FunctionReturn(fun) => {
                    let ty = self.db.function_data(fun).return_ty.clone();
                    Ty::FunctionVar { ty, fun, arg: None }
                }
            },

            Expr::UnaryOp { arg, op: UnaryOp::Identity } => self.infere_expr(stmt, arg)?,
            Expr::UnaryOp { arg, op: UnaryOp::Neg } => {
                let ty = self.infere_expr(stmt, arg)?;
                let which = self.expect::<false>(
                    arg,
                    Some(expr),
                    ty,
                    Cow::Borrowed(&[
                        TyRequirement::Val(Type::Integer),
                        TyRequirement::Val(Type::Real),
                    ]),
                )?;
                let ty = match which {
                    0 => Type::Integer,
                    1 => Type::Real,
                    _ => unreachable!(),
                };
                Ty::Val(ty)
            }
            Expr::UnaryOp { arg, op: UnaryOp::BitNegate } => {
                let ty = self.infere_expr(stmt, arg)?;
                // TODO bool
                self.expect::<false>(
                    arg,
                    Some(expr),
                    ty,
                    Cow::Borrowed(&[TyRequirement::Val(Type::Integer)]),
                );
                Ty::Val(Type::Integer)
            }
            Expr::UnaryOp { arg, op: UnaryOp::Not } => {
                let ty = self.infere_expr(stmt, arg)?;
                self.expect::<false>(
                    arg,
                    Some(expr),
                    ty,
                    Cow::Borrowed(&[TyRequirement::Condition]),
                );
                Ty::Val(Type::Bool)
            }

            // TODO(JW): properly deal with this, maybe this
            // should have been handled during ast validation
            Expr::BinaryOp { lhs, rhs, op: None } => {
                self.infere_expr(stmt, lhs);
                self.infere_expr(stmt, rhs);
                return None;
            }
            Expr::BinaryOp { lhs, rhs, op: Some(op) } => {
                self.infere_bin_op(stmt, expr, lhs, rhs, op)?
            }

            Expr::Select { cond, then_val, else_val } => {
                self.infere_cond(stmt, cond);
                self.infere_fun_args(
                    stmt,
                    expr,
                    &[then_val, else_val],
                    Cow::Borrowed(TiSlice::from_ref(SignatureData::SELECT_OP)),
                    None,
                )
                .0?
            }

            Expr::Call { ref fun, ref args } => {
                self.infere_fun_call(stmt, expr, fun.as_ref()?, args)?
            }

            Expr::Literal(Literal::Int(_)) => Ty::Literal(Type::Integer),
            Expr::Literal(Literal::Float(_)) => Ty::Literal(Type::Real),
            // +/- inf can only appear in param bounds.
            // This is checked during ast validation and when it appears it is always correct
            Expr::Literal(Literal::Inf) => {
                if let Some(ty) = &self.expr_stmt_ty {
                    self.result.expr_types[expr] = Ty::Val(ty.clone());
                }
                return None;
            }
            Expr::Literal(Literal::String(_)) => Ty::Literal(Type::String),

            Expr::Array(ref args) if args.is_empty() => Ty::Val(Type::EmptyArray),
            Expr::Array(ref args) => self.infere_array(stmt, args)?,
        };

        self.result.expr_types[expr] = ty.clone();
        Some(ty)
    }

    fn infere_bin_op(
        &mut self,
        stmt: StmtId,
        expr: ExprId,
        lhs: ExprId,
        rhs: ExprId,
        op: BinaryOp,
    ) -> Option<Ty> {
        let signatures = match op {
            BinaryOp::BooleanOr | BinaryOp::BooleanAnd => &[SignatureData::CONDITIONAL_BIN_OP],

            BinaryOp::Addition
            | BinaryOp::Subtraction
            | BinaryOp::Multiplication
            | BinaryOp::Division
            | BinaryOp::Remainder => SignatureData::NUMERIC_BIN_OP,

            BinaryOp::LeftShift
            | BinaryOp::RightShift
            | BinaryOp::BitwiseXor
            | BinaryOp::BitwiseXnor
            | BinaryOp::BitwiseOr
            | BinaryOp::BitwiseAnd => &[SignatureData::INT_BIN_OP],

            BinaryOp::Power => &[SignatureData::REAL_BIN_OP],

            BinaryOp::LesserEqualTest
            | BinaryOp::GreaterEqualTest
            | BinaryOp::LesserTest
            | BinaryOp::GreaterTest => SignatureData::NUMERIC_COMPARISON,

            BinaryOp::EqualityTest | BinaryOp::NegatedEqualityTest => SignatureData::ANY_COMPARISON,
        };
        let signatures = Cow::Borrowed(TiSlice::from_ref(signatures));

        self.infere_fun_args(stmt, expr, &[lhs, rhs], signatures, None).0
    }

    fn infere_cond(&mut self, stmt: StmtId, expr: ExprId) {
        if let Some(ty) = self.infere_expr(stmt, expr) {
            self.expect::<false>(expr, None, ty, Cow::Borrowed(&[TyRequirement::Condition]));
        }
    }

    fn infere_fun_call(
        &mut self,
        stmt: StmtId,
        expr: ExprId,
        fun: &Path,
        args: &[ExprId],
    ) -> Option<Ty> {
        let def = self.resolve_path(stmt, expr, fun)?;
        match def {
            ScopeItem::NatureAccess(access) => {
                self.infere_nature_access(stmt, expr, access, args);
                Some(Ty::Val(Type::Real))
            }
            ScopeItem::BuiltIn(builtin) => {
                self.result.resolved_calls.insert(expr, ResolvedFun::BuiltIn(builtin));
                self.infere_builtin(stmt, expr, builtin, args).0
            }
            ScopeItem::FunctionId(fun) => self.infere_user_fun_call(stmt, expr, fun, args),
            ScopeItem::ParamSysFun(param) => {
                self.result.resolved_calls.insert(expr, ResolvedFun::Param(param));
                if !args.is_empty() {
                    self.result.diagnostics.push(InferDiagnostic::ArgCntMismatch {
                        expected: 0,
                        found: args.len(),
                        expr,
                        exact: true,
                    });
                }
                Some(Ty::Val(Type::Real))
            }
            found => {
                self.result.diagnostics.push(InferDiagnostic::PathResolveError {
                    err: PathResolveError::ExpectedItemKind {
                        expected: "a function",
                        found: ResolvedPath::ScopeItem(found),
                        name: fun.segments.last().unwrap().to_owned(),
                    },
                    expr,
                });
                None
            }
        }
    }

    fn infere_nature_access(
        &mut self,
        stmt: StmtId,
        expr: ExprId,
        access: NatureAccess,
        args: &[ExprId],
    ) {
        // resolve as flow first because we don't yet know if this is a flow or pot access
        // This choice is arbitrary (but must be consistent with the code below)
        if !self.infere_builtin(stmt, expr, BuiltIn::flow, args).1 {
            return;
        }

        let nature = access.0.lookup(self.db.upcast()).nature;
        // Now that we know that the arguments are valid, actually resolve whether this is flow or
        // pot access
        let access = self.infere_access_kind(nature, expr, args[0]);

        // update resolved_calls in case this is actually a pot and not a flow access
        match access {
            Some(DisciplineAccess::Potential) => {
                self.result.resolved_calls.insert(expr, ResolvedFun::BuiltIn(BuiltIn::potential));
            }
            Some(DisciplineAccess::Flow) => {
                self.result.resolved_calls.insert(expr, ResolvedFun::BuiltIn(BuiltIn::flow));
            }
            None => {
                self.result.resolved_calls.insert(expr, ResolvedFun::InvalidNatureAccess(nature));
            }
        }
    }

    fn infere_access_kind(
        &self,
        nature: NatureId,
        expr: ExprId,
        arg: ExprId,
    ) -> Option<DisciplineAccess> {
        let signature = self.result.resolved_signatures.get(&expr)?;
        let node = match *signature {
            NATURE_ACCESS_BRANCH => {
                let branch = self.result.expr_types[arg].unwrap_branch();
                let branch_info = self.db.branch_info(branch)?;
                return branch_info.access(nature, self.db);
            }
            NATURE_ACCESS_NODES | NATURE_ACCESS_NODE_GND => {
                self.result.expr_types[arg].unwrap_node()
            }
            NATURE_ACCESS_PORT_FLOW => self.result.expr_types[arg].unwrap_port_flow(),
            _ => unreachable!(),
        };

        let discipline = self.db.node_discipline(node)?;
        self.db.discipline_info(discipline).access(nature, self.db)
    }

    fn infere_builtin(
        &mut self,
        stmt: StmtId,
        expr: ExprId,
        builtin: BuiltIn,
        args: &[ExprId],
    ) -> (Option<Ty>, bool) {
        let BuiltinInfo { signatures, min_args, max_args, .. } = builtin.into();

        // argument count mismatch
        let exact = Some(min_args) == max_args;
        if args.len() < min_args || max_args.is_some_and(|it| it < args.len()) {
            self.result.diagnostics.push(InferDiagnostic::ArgCntMismatch {
                expected: min_args,
                found: args.len(),
                expr,
                exact,
            });
            return (default_return_ty(signatures), false);
        }

        // ddx needs some special treatment
        if let BuiltIn::ddx = builtin {
            self.infere_ddx(stmt, expr, args[0], args[1]);
            return (Some(Ty::Val(Type::Real)), true);
        }

        // function signature mismatch
        let mut infere_args = args;
        let infere_signatures = match builtin {
            BuiltIn::limit => {
                if args.len() >= 2 {
                    infere_args = &args[0..2];
                }
                Cow::Borrowed(TiSlice::from_ref(signatures))
            }
            _ => {
                if max_args.is_some() {
                    // normal functions
                    Cow::Borrowed(TiSlice::from_ref(signatures))
                } else {
                    // vararg functions
                    let mut signatures = Vec::from(signatures);
                    for sig in &mut signatures {
                        sig.args.to_mut().resize(args.len(), TyRequirement::AnyVal)
                    }
                    Cow::Owned(TiVec::from(signatures))
                }
            }
        };
        debug_assert_ne!(&infere_signatures.raw, &[]);

        let (Some(ty), valid) =
            self.infere_fun_args(stmt, expr, infere_args, infere_signatures, None)
        else {
            return (default_return_ty(signatures), false);
        };

        match builtin {
            BuiltIn::write
            | BuiltIn::display
            | BuiltIn::strobe
            | BuiltIn::monitor
            | BuiltIn::debug
            | BuiltIn::warning
            | BuiltIn::error
            | BuiltIn::info
            | BuiltIn::fatal => self.infere_display(stmt, args),
            BuiltIn::limit => self.infere_limit(stmt, expr, args),
            _ => (),
        }

        (Some(ty), valid)
    }

    fn infere_user_fun_call(
        &mut self,
        stmt: StmtId,
        expr: ExprId,
        func: FunctionId,
        args: &[ExprId],
    ) -> Option<Ty> {
        self.result.resolved_calls.insert(expr, ResolvedFun::User { func, limit: false });
        let fun_info = self.db.function_data(func);
        if fun_info.args.len() != args.len() {
            self.result.diagnostics.push(InferDiagnostic::ArgCntMismatch {
                expected: fun_info.args.len(),
                found: args.len(),
                expr,
                exact: true,
            });
            return Some(Ty::Val(fun_info.return_ty.clone()));
        }

        let signature = fun_info
            .args
            .iter()
            .map(|arg| {
                if arg.is_output {
                    // Output arguments must be variables
                    TyRequirement::Var(arg.ty.clone())
                } else {
                    TyRequirement::Val(arg.ty.clone())
                }
            })
            .collect();

        self.infere_fun_args(
            stmt,
            expr,
            args,
            Cow::Owned(TiVec::from(vec![SignatureData {
                args: Cow::Owned(signature),
                return_ty: fun_info.return_ty.clone(),
            }])),
            Some(func),
        )
        .0
    }

    fn infere_limit(&mut self, stmt: StmtId, expr: ExprId, args: &[ExprId]) {
        let Some(sig) = self.result.resolved_signatures.get(&expr) else {
            return; // already reported an error no need to repeat
        };

        let probe = args[0];
        if self.result.expr_types[probe] != Ty::Val(Type::Err)
            && !matches!(
                self.result.resolved_calls.get(&probe),
                Some(ResolvedFun::BuiltIn(BuiltIn::potential | BuiltIn::flow))
            )
        {
            self.result.diagnostics.push(InferDiagnostic::ExpectedProbe { expr: probe })
        }

        if args.len() == 1 {
            // only one argument (no limit function specified)
        } else if let Some(Ty::UserFunction(func)) = self.result.expr_types.get(args[1]).cloned() {
            debug_assert_eq!(*sig, LIMIT_USER_FUNCTION);
            let fun_info = self.db.function_data(func);

            // user-function needs two extra arguments but $limit also accepts two arguments that are
            // not passed directly to the function so these must just be equal
            if fun_info.args.len() != args.len() {
                self.result.diagnostics.push(InferDiagnostic::ArgCntMismatch {
                    expected: fun_info.args.len(),
                    found: args.len(),
                    expr,
                    exact: true,
                });
                return;
            }

            let output_args: Vec<_> = fun_info
                .args
                .iter_enumerated()
                .filter_map(|(id, info)| info.is_output.then_some(id))
                .collect();

            let invalid_ret = !matches!(fun_info.return_ty, Type::Real | Type::Err);
            let invalid_arg0 = !matches!(fun_info.args.raw[0].ty, Type::Real | Type::Err);
            let invalid_arg1 = !matches!(fun_info.args.raw[1].ty, Type::Real | Type::Err);

            if invalid_ret || invalid_arg0 || invalid_arg1 || !output_args.is_empty() {
                self.result.diagnostics.push(InferDiagnostic::InvalidLimitFunction {
                    expr,
                    func,
                    invalid_arg0,
                    invalid_arg1,
                    invalid_ret,
                    output_args,
                })
            }

            let signature = fun_info.args.raw[2..]
                .iter()
                .map(|arg| TyRequirement::Val(arg.ty.clone()))
                .collect();

            self.infere_fun_args(
                stmt,
                expr,
                &args[2..],
                Cow::Owned(TiVec::from(vec![SignatureData {
                    args: Cow::Owned(signature),
                    return_ty: fun_info.return_ty.clone(),
                }])),
                Some(func),
            );

            self.result.resolved_calls.insert(expr, ResolvedFun::User { func, limit: true });
        } else if *sig == LIMIT_BUILTIN_FUNCTION {
            self.infere_fun_args(
                stmt,
                expr,
                &args[2..],
                Cow::Owned(TiVec::from(vec![SignatureData {
                    args: Cow::Owned(vec![TyRequirement::Val(Type::Real); args.len() - 2]),
                    return_ty: Type::Real,
                }])),
                None,
            );
        }
    }

    /// # Note
    /// The argument count needs checking, otherwise useless error messages will be produced
    fn infere_fun_args(
        &mut self,
        stmt: StmtId,
        expr: ExprId,
        args: &[ExprId],
        signatures: Cow<'static, TiSlice<Signature, SignatureData>>,
        src: Option<FunctionId>,
    ) -> (Option<Ty>, bool) {
        debug_assert!(signatures.iter().any(|sig| sig.args.len() == args.len()));
        let arg_types: Vec<_> = args.iter().map(|arg| self.infere_expr(stmt, *arg)).collect();
        let mut is_valid = true;
        let mut candidates: Vec<_> = signatures.keys().collect();
        let mut new_candidates = Vec::new();
        let mut errors = Vec::new();

        for (i, (arg, ty)) in zip(args, &arg_types).enumerate() {
            if let Some(ty) = ty {
                new_candidates.clone_from(&candidates);
                new_candidates.retain(|candidate| {
                    signatures[*candidate]
                        .args
                        .get(i)
                        .is_some_and(|req| ty.satisfies_with_conversion(req))
                });
                if new_candidates.is_empty() {
                    let candidate_types: Vec<TyRequirement> = candidates
                        .iter()
                        .filter_map(|candidate| signatures[*candidate].args.get(i).cloned())
                        .fold(Vec::new(), |mut acc, x| {
                            // JW: dedup candidate types for better UI
                            if !acc.contains(&x) {
                                acc.push(x);
                            }
                            acc
                        });
                    debug_assert_ne!(&candidate_types, &[]);
                    errors.push(TypeMismatch {
                        expected: Cow::from(candidate_types),
                        found_ty: ty.clone(),
                        expr: *arg,
                    });
                } else {
                    mem::swap(&mut new_candidates, &mut candidates)
                }
            } else {
                is_valid = false;
            }
        }
        candidates.retain(|sig| signatures[*sig].args.len() == args.len());

        if !errors.is_empty() || candidates.is_empty() {
            self.result.diagnostics.push(
                SignatureMismatch {
                    type_mismatches: errors.into_boxed_slice(),
                    signatures: signatures.clone(),
                    src,
                    found: arg_types
                        .iter()
                        .map(|it| it.clone().unwrap_or(Ty::Val(Type::Err)))
                        .collect(),
                }
                .into(),
            );
            return (default_return_ty(&signatures.raw), false);
        }

        let sig = match candidates.as_slice() {
            [] => unreachable!(),
            [sig] => *sig,
            _ if arg_types.iter().any(|ty| ty.is_none()) => {
                return (default_return_ty(&signatures.raw), false)
            }
            _ => {
                new_candidates.clone_from(&candidates);
                candidates.retain(|candidate| {
                    zip(&arg_types, signatures[*candidate].args.as_ref())
                        .all(|(ty, req)| ty.as_ref().is_some_and(|ty| ty.satisfies_semantic(req)))
                });
                if candidates.len() > 1 {
                    new_candidates.clone_from(&candidates);
                    candidates.retain(|candidate| {
                        zip(&arg_types, signatures[*candidate].args.as_ref())
                            .all(|(ty, req)| ty.as_ref().is_some_and(|ty| ty.satisfies_exact(req)))
                    });
                    if candidates.is_empty() {
                        candidates = new_candidates;
                    }
                }

                candidates[0]
            }
        };

        for (dst, (src, arg)) in zip(signatures[sig].args.as_ref(), zip(arg_types, args)) {
            if let Some(src) = src.and_then(|ty| ty.to_value()) {
                if let Some(cast) = dst.cast(&src) {
                    self.result.casts.insert(*arg, cast);
                }
            }
        }

        if signatures.len() > 1 {
            self.result.resolved_signatures.insert(expr, sig);
        }

        (Some(Ty::Val(signatures[sig].return_ty.clone())), is_valid)
    }

    fn infere_ddx(&mut self, stmt: StmtId, expr: ExprId, val: ExprId, unknown: ExprId) {
        // the first arg should be real type
        if let Some(ty) = self.infere_expr(stmt, val) {
            self.expect::<false>(expr, None, ty, Cow::Borrowed(&[TyRequirement::Val(Type::Real)]));
        }
        // [LRM 4.5.6]: The second argument of ddx() shall be the potential of a scalar
        // net or port or the flow through a branch, because these are the unknown variables
        // in the system of equations for the analog solver.
        let ty = self.infere_expr(stmt, unknown);
        if ty.is_some() {
            let (fun, sig) = match (
                self.result.resolved_calls.get(&unknown),
                self.result.resolved_signatures.get(&unknown),
            ) {
                (Some(ResolvedFun::BuiltIn(fun)), Some(sig)) => (*fun, *sig),
                (Some(ResolvedFun::BuiltIn(fun)), None) => (*fun, DDX_TEMP),
                _ => {
                    let diag = InferDiagnostic::InvalidUnknown { expr: unknown };
                    self.result.diagnostics.push(diag);
                    return;
                }
            };

            let signature = match (fun, sig) {
                (BuiltIn::potential, NATURE_ACCESS_NODE_GND) => DDX_POT,
                (BuiltIn::flow, NATURE_ACCESS_BRANCH | NATURE_ACCESS_NODES) => DDX_FLOW,
                (BuiltIn::potential, NATURE_ACCESS_NODES) => {
                    self.result
                        .diagnostics
                        .push(InferDiagnostic::NonStandardUnknown { expr: unknown, stmt });
                    DDX_POT_DIFF
                }
                (BuiltIn::temperature, DDX_TEMP) => {
                    self.result
                        .diagnostics
                        .push(InferDiagnostic::NonStandardUnknown { expr: unknown, stmt });
                    DDX_TEMP
                }
                _ => {
                    self.result.diagnostics.push(InferDiagnostic::InvalidUnknown { expr: unknown });
                    return;
                }
            };

            self.result.resolved_signatures.insert(expr, signature);
        }
    }

    fn infere_array(&mut self, stmt: StmtId, args: &[ExprId]) -> Option<Ty> {
        let infere_value_ty = |sel: &mut Self, arg| -> Option<Type> {
            sel.infere_expr(stmt, arg).and_then(|ty| {
                let res = ty.to_value();
                if res.is_none() {
                    sel.result.diagnostics.push(
                        TypeMismatch {
                            expected: Cow::Borrowed(&[TyRequirement::AnyVal]),
                            found_ty: ty,
                            expr: arg,
                        }
                        .into(),
                    )
                }
                res
            })
        };

        let mut iter = args.iter();
        let (ty, first_expr) = loop {
            let arg = iter.next()?;

            if let Some(ty) = infere_value_ty(self, *arg) {
                break (ty, *arg);
            }
        };

        let ty = iter.fold(ty, |ty, arg| {
            let arg_ty = match infere_value_ty(self, *arg) {
                Some(arg_ty) => arg_ty,
                None => return ty,
            };
            match ty.union(&arg_ty) {
                Some(ty) => ty,
                None => {
                    self.result.diagnostics.push(InferDiagnostic::ArrayTypeMismatch {
                        expected: ty.clone(),
                        found_ty: arg_ty,
                        found_expr: *arg,
                        expected_expr: first_expr,
                    });
                    ty
                }
            }
        });

        for arg in args {
            if let Some(arg_ty) = self.result.expr_types[*arg].to_value() {
                if arg_ty != ty {
                    self.result.casts.insert(*arg, ty.clone());
                }
            }
        }

        Some(Ty::Val(ty))
    }

    fn infere_display(&mut self, stmt: StmtId, args: &[ExprId]) {
        let mut i = 0;
        while let Some(fmt_expr) = args.get(i) {
            i += 1;
            if let Expr::Literal(Literal::String(ref lit)) = self.body.exprs[*fmt_expr] {
                let mut chars = lit.char_indices();
                while let Some((start, c)) = chars.next() {
                    if c == '%' {
                        let pos = chars.next();
                        let mut end: TextSize = (start + 2).try_into().unwrap();
                        let ty = match pos.map(|(_, c)| c) {
                            Some('%' | 'm' | 'M' | 'l' | 'L') => continue, // escape sequences, always correct
                            Some('d' | 'D' | 'h' | 'H' | 'o' | 'O' | 'b' | 'B' | 'c' | 'C') => {
                                Type::Integer
                            }
                            Some('s' | 'S') => Type::String,
                            _ => {
                                let res =
                                    parse_real_fmt_spec(start as u32, *fmt_expr, pos, &mut chars);
                                if let Some(err) = res.err {
                                    self.result.diagnostics.push(err);
                                    i += 1 + res.dynamic_args.len();
                                    continue;
                                }

                                for pos in res.dynamic_args {
                                    self.check_display_dynamic_arg(
                                        *fmt_expr,
                                        args.get(i).copied(),
                                        pos,
                                    );
                                    i += 1;
                                }

                                end = res.end;
                                Type::Real
                            }
                        };

                        let arg = args.get(i).copied();
                        let range = TextRange::new(start.try_into().unwrap(), end);
                        self.check_display_arg_val(stmt, *fmt_expr, arg, range, ty);

                        i += 1;
                    }
                }
            }
        }
    }

    fn check_display_dynamic_arg(&mut self, fmt_expr: ExprId, arg: Option<ExprId>, off: TextSize) {
        let arg = if let Some(arg) = arg {
            arg
        } else {
            self.result.diagnostics.push(InferDiagnostic::MissingFmtArg {
                fmt_lit: fmt_expr,
                lit_range: TextRange::at(off, 1u32.into()),
            });

            return;
        };
        match self.result.expr_types[arg].to_value() {
            Some(Type::Integer) => (),

            Some(ty) if ty.is_convertible_to(&Type::Integer) => {
                self.result.casts.insert(arg, Type::Integer);
            }
            _ => self.result.diagnostics.push(InferDiagnostic::DisplayTypeMismatch {
                err: TypeMismatch {
                    expected: Cow::Borrowed(&[TyRequirement::Val(Type::Integer)]),
                    found_ty: self.result.expr_types[arg].clone(),
                    expr: arg,
                },
                fmt_lit: fmt_expr,
                lit_range: TextRange::at(off, 1u32.into()),
                lint_ctx: None,
            }),
        }
    }

    fn check_display_arg_val(
        &mut self,
        stmt: StmtId,
        fmt_expr: ExprId,
        arg: Option<ExprId>,
        lit_range: TextRange,
        ty: Type,
    ) {
        let Some(arg) = arg else {
            self.result
                .diagnostics
                .push(InferDiagnostic::MissingFmtArg { fmt_lit: fmt_expr, lit_range });
            return;
        };
        match self.result.expr_types[arg].to_value() {
            Some(ty_) if ty_ == ty => (),

            Some(ty_) if ty_.is_convertible_to(&ty) => {
                self.result.casts.insert(arg, ty);
            }

            Some(ty_) if ty_.is_assignable_to(&ty) => {
                self.result.casts.insert(arg, ty.clone());
                self.result.diagnostics.push(InferDiagnostic::DisplayTypeMismatch {
                    err: TypeMismatch {
                        expected: Cow::Owned(vec![TyRequirement::Val(ty)]),
                        found_ty: self.result.expr_types[arg].clone(),
                        expr: arg,
                    },
                    fmt_lit: fmt_expr,
                    lit_range,
                    lint_ctx: Some(stmt),
                })
            }
            _ => self.result.diagnostics.push(InferDiagnostic::DisplayTypeMismatch {
                err: TypeMismatch {
                    expected: Cow::Owned(vec![TyRequirement::Val(ty)]),
                    found_ty: self.result.expr_types[arg].clone(),
                    expr: arg,
                },
                fmt_lit: fmt_expr,
                lit_range,
                lint_ctx: None,
            }),
        }
    }

    fn expect<const SEMANTIC: bool>(
        &mut self,
        expr: ExprId,
        parent: Option<ExprId>,
        ty: Ty,
        req: Cow<'static, [TyRequirement]>,
    ) -> Option<usize> {
        let satisfies =
            if SEMANTIC { Ty::satisfies_semantic } else { Ty::satisfies_with_conversion };
        let which = req.iter().position(|req| satisfies(&ty, req));
        match which {
            Some(matched) => {
                if !SEMANTIC {
                    if let Some(expr) = parent {
                        self.result.resolved_signatures.insert(expr, Signature::from(matched));
                    }
                    if let Some(ty) = ty.to_value() {
                        if let Some(cast) = req[matched].cast(&ty) {
                            self.result.casts.insert(expr, cast);
                        }
                    }
                }
            }
            None => self
                .result
                .diagnostics
                .push(TypeMismatch { expected: req, found_ty: ty, expr }.into()),
        }
        which
    }

    fn resolve_item_path<T: ScopeItemKind>(
        &mut self,
        stmt: StmtId,
        expr: ExprId,
        path: &Path,
    ) -> Option<T> {
        match self.body.stmt_scopes[stmt].resolve_item_path(self.db.upcast(), path) {
            Ok(item) => Some(item),
            Err(err) => {
                self.result.diagnostics.push(InferDiagnostic::PathResolveError { err, expr });
                None
            }
        }
    }

    fn resolve_path(&mut self, stmt: StmtId, expr: ExprId, path: &Path) -> Option<ScopeItem> {
        let resolved_path = match self.body.stmt_scopes[stmt].resolve_path(self.db.upcast(), path) {
            Ok(resolved_path) => resolved_path,
            Err(err) => {
                self.result.diagnostics.push(InferDiagnostic::PathResolveError { err, expr });
                return None;
            }
        };

        let attr = match resolved_path {
            ResolvedPath::FlowAttr { branch, ref name } => {
                BranchTy::flow_attr(self.db, branch, name)?
            }
            ResolvedPath::PotentialAttr { branch, ref name } => {
                BranchTy::potential_attr(self.db, branch, name)?
            }
            ResolvedPath::ScopeItem(def) => return Some(def),
        };

        match attr {
            Ok(attr) => Some(attr.into()),
            Err(err) => {
                self.result.diagnostics.push(InferDiagnostic::PathResolveError { err, expr });
                None
            }
        }
    }
}

fn default_return_ty(signatures: &[SignatureData]) -> Option<Ty> {
    let ty = &signatures.first()?.return_ty;
    signatures.iter().all(|sig| &sig.return_ty == ty).then(|| Ty::Val(ty.clone()))
}
