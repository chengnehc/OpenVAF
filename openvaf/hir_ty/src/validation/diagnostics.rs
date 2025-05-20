use basedb::{
    diagnostics::{Diagnostic, Label, LabelStyle, Report},
    lints::{self, Lint, LintSrc},
    AstId, BaseDB, FileId,
};
use hir_def::{
    body::{BodySourceMap, ExprId, StmtId},
    BuiltIn, FunctionArgLoc, ItemTreeNode, Lookup, NatureId, NodeId, ParamId, VarId,
};
use syntax::{
    ast,
    sourcemap::{FileSpan, SourceMap},
    Name, Parse, SourceFile, TextRange,
};

use crate::db::HirTyDB;
use crate::inference::BranchWrite;

mod body;
mod ty;

use super::BodyContext;

/* Body expressions and statements diagnostics */
pub struct BodyDiagnosticWrapped<'a> {
    pub db: &'a dyn HirTyDB,
    pub diag: &'a BodyDiagnostic,
    pub body_src_map: &'a BodySourceMap,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum BodyDiagnostic {
    /* Port*/
    ExpectedPort {
        expr: ExprId,
        node: NodeId,
    },

    /* Branch access */
    PotentialOfPortFlow {
        expr: ExprId,
        branch: Option<BranchId>,
    },
    TrivialBranchAccess {
        branch: BranchWrite,
        expr: ExprId,
        stmt: StmtId,
    },
    IncompatibleUnnamedBranch {
        expr: ExprId,
        node1: NodeId,
        node2: NodeId,
    },
    IllegalNatureAccess {
        is_pot: bool,
        expr: ExprId,
    },
    IncompatibleNatureAccess {
        candidates: [Option<(Name, Name)>; 2],
        access_nature: Option<NatureId>,
        access_expr: ExprId,
        branch: String,
    },

    /* Context violation */
    IllegalContribute {
        stmt: StmtId,
        ctxt: BodyContext,
    },
    IllegalCtxtAccess {
        kind: IllegalCtxtAccessKind,
        ctxt: BodyContext,
        expr: ExprId,
    },

    /* Parameter */
    IllegalParamAccess {
        def: ParamId,
        expr: ExprId,
        param: ParamId,
    },

    /* User function */
    WriteToInputArg {
        expr: ExprId,
        arg: FunctionArgLoc,
    },

    /* Buitin function */
    UnsupportedFunction {
        expr: ExprId,
        func: BuiltIn,
    },
    ConstSimparam {
        known: bool,
        expr: ExprId,
        stmt: StmtId,
    },
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum IllegalCtxtAccessKind {
    NatureAccess,
    AnalogOperator { name: Name, is_standard: bool, non_const_dominator: Box<[ExprId]> },
    AnalysisFun { name: Name },
    Var(VarId),
}

/* Type Diagnostics */

use basedb::ErasedAstId;
use hir_def::{
    nameres::PathResolveError, BranchId, DisciplineId, LocalDisciplineAttrId, LocalNatureAttrId,
};

pub struct TypeDiagnosticWrapped<'a> {
    pub db: &'a dyn HirTyDB,
    pub diag: &'a TypeDiagnostic,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum TypeDiagnostic {
    /* Name and Path */
    PathError { err: PathResolveError, range: TextRange },

    /* Nature and Discipline */
    DuplicateNatureAttr(DuplicateItem<LocalNatureAttrId, NatureId>),
    DuplicateDisciplineAttr(DuplicateItem<LocalDisciplineAttrId, DisciplineId>),

    /* Node and Port */
    PortWithoutDirection { decl: ErasedAstId, name: Name },
    NodeWithoutDiscipline { decl: ErasedAstId, name: Name },
    MultipleDirections(DuplicateItem<ErasedAstId, NodeId>),
    MultipleDisciplines(DuplicateItem<ErasedAstId, NodeId>),
    MultipleGnds(DuplicateItem<ErasedAstId, NodeId>),

    /* Branch */
    ExpectedPort { node: NodeId, src: ErasedAstId },
    IncompatibleBranch { branch: BranchId, node1: NodeId, node2: NodeId },

    /* Function */
    MultipleFuncArgBind(DuplicateItem<AstId<ast::Var>, Name>),
    FuncArgWithoutVarBind { decl: AstId<ast::FunctionArg>, name: Name },
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct DuplicateItem<Item, Def> {
    pub src: Def,
    pub first: Item,
    pub subsequent: Vec<Item>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
struct IncompatibleBranchDiagnostic {
    branch_span: FileSpan,
    branch_name: String,
    node1: NodeId,
    node2: NodeId,
}
