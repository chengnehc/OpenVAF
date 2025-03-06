use std::borrow::Cow;
use std::ops::Deref;
use stdx::{impl_display, impl_from, pretty};

use hir_def::nameres::PathResolveError;
use hir_def::{ExprId, FunctionId, LocalFunctionArgId, StmtId, Type};
use syntax::{ast, TextRange};
use typed_index_collections::TiSlice;

use crate::types::{Signature, SignatureData, Ty, TyRequirement};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferDiagnostic {
    InvalidAssignDst {
        e: ExprId,
        maybe_different_operand: Option<ast::AssignOp>,
        assignment_kind: ast::AssignOp,
    },
    PathResolveError {
        err: PathResolveError,
        expr: ExprId,
    },
    ArgCntMismatch {
        expected: usize,
        found: usize,
        expr: ExprId,
        exact: bool,
    },
    ExpectedProbe {
        e: ExprId,
    },
    InvalidLimitFunction {
        expr: ExprId,
        func: FunctionId,
        invalid_arg0: bool,
        invalid_arg1: bool,
        invalid_ret: bool,
        output_args: Vec<LocalFunctionArgId>,
    },
    DisplayTypeMismatch {
        err: TypeMismatch,
        fmt_lit: ExprId,
        lit_range: TextRange,
        lint_ctx: Option<StmtId>,
    },
    MissingFmtArg {
        fmt_lit: ExprId,
        lit_range: TextRange,
    },
    InvalidFmtSpecifierChar {
        fmt_lit: ExprId,
        lit_range: TextRange,
        err_char: char,
        candidates: &'static [char],
    },
    InvalidFmtSpecifierEnd {
        fmt_lit: ExprId,
        lit_range: TextRange,
    },
    TypeMismatch(TypeMismatch),
    SignatureMismatch(SignatureMismatch),
    ArrayTypeMismatch {
        expected: Type,
        found_ty: Type,
        found_expr: ExprId,
        expected_expr: ExprId,
    },
    InvalidUnknown {
        e: ExprId,
    },
    NonStandardUnknown {
        e: ExprId,
        stmt: StmtId,
    },
}

impl_from!(TypeMismatch, SignatureMismatch for InferDiagnostic);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeMismatch {
    pub expected: Cow<'static, [TyRequirement]>,
    pub found_ty: Ty,
    pub expr: ExprId,
}
impl_display! {
    match TypeMismatch{
        TypeMismatch{expected, ..} => "expected {}", pretty::List::new(expected.deref());
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureMismatch {
    pub type_mismatches: Box<[TypeMismatch]>,
    pub signatures: Cow<'static, TiSlice<Signature, SignatureData>>,
    pub src: Option<FunctionId>,
    pub found: Box<[Ty]>,
}
