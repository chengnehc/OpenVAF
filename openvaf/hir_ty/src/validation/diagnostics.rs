use basedb::{
    diagnostics::{Diagnostic, Label, LabelStyle, Report},
    lints::{self, Lint, LintSrc},
    BaseDB, FileId,
};
use hir_def::{
    body::{BodySourceMap, ExprId, StmtId},
    BuiltIn, FunctionArgLoc, ItemTreeNode, Lookup, NatureId, NodeId, ParamId, VarId,
};
use syntax::{
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
    pub body_sm: &'a BodySourceMap,
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
        access_expr: ExprId,
        node1: NodeId,
        node2: NodeId,
    },
    IllegalNatureAccess {
        is_pot: bool,
        access_expr: ExprId,
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

    /* Function */
    WriteToInputArg {
        expr: ExprId,
        arg: FunctionArgLoc,
    },
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

use basedb::{AstId, ErasedAstId};
use hir_def::{
    nameres::PathResolveError, BranchId, DisciplineId, ItemTree, LocalDisciplineAttrId,
    LocalNatureAttrId,
};
use syntax::{ast, SyntaxNodePtr};

pub struct TypeDiagnosticWrapped<'a> {
    pub db: &'a dyn HirTyDB,
    pub item_tree: &'a ItemTree,
    pub diag: &'a TypeDiagnostic,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum TypeDiagnostic {
    PathError { err: PathResolveError, src: SyntaxNodePtr },
    DuplicateNatureAttr(DuplicateItem<LocalNatureAttrId, NatureId>),
    DuplicateDisciplineAttr(DuplicateItem<LocalDisciplineAttrId, DisciplineId>),
    PortWithoutDirection { decl: ErasedAstId, name: Name },
    NodeWithoutDiscipline { decl: ErasedAstId, name: Name },
    MultipleDirections(DuplicateItem<AstId<ast::PortDecl>, NodeId>),
    MultipleDisciplines(DuplicateItem<ErasedAstId, NodeId>),
    MultipleGnds(DuplicateItem<ErasedAstId, NodeId>),
    ExpectedPort { node: NodeId, src: ErasedAstId },
    IncompatibleBranch { branch: BranchId, node1: NodeId, node2: NodeId },
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
