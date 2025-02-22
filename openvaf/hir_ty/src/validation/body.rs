//use stdx::impl_display;

use ahash::{HashMap, HashSet};
use hir_def::{
    BranchId, BuiltIn, DefWithBodyId, ExprId, FunctionArgLoc, NatureId, NodeId, ParamId, StmtId,
    VarId,
};
use syntax::name::Name;

use super::{BodyCtx, BodyValidator};
use crate::db::HirTyDB;
use crate::inference::BranchWrite;

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum BodyDiagnostic {
    ExpectedPort {
        expr: ExprId,
        node: NodeId,
    },
    TrivialBranchAccess {
        branch: BranchWrite,
        expr: ExprId,
        stmt: StmtId,
    },
    PotentialOfPortFlow {
        expr: ExprId,
        branch: Option<BranchId>,
    },
    IllegalContribute {
        stmt: StmtId,
        ctx: BodyCtx,
    },
    WriteToInputArg {
        expr: ExprId,
        arg: FunctionArgLoc,
    },
    IllegalParamAccess {
        def: ParamId,
        expr: ExprId,
        param: ParamId,
    },
    IllegalCtxAccess {
        kind: IllegalCtxAccessKind,
        ctx: BodyCtx,
        expr: ExprId,
    },
    ConstSimparam {
        known: bool,
        expr: ExprId,
        stmt: StmtId,
    },
    UnsupportedFunction {
        expr: ExprId,
        func: BuiltIn,
    },
    IncompatibleNatureAccess {
        candidates: [Option<(Name, Name)>; 2],
        access_nature: Option<NatureId>,
        access_expr: ExprId,
        branch: String,
    },
    IllegalNatureAccess {
        is_pot: bool,
        access_expr: ExprId,
    },
    IncompatibleImplicitBranch {
        access_expr: ExprId,
        node1: NodeId,
        node2: NodeId,
    },
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum IllegalCtxAccessKind {
    NatureAccess,
    AnalogOperator { name: Name, is_standard: bool, non_const_dominator: Box<[ExprId]> },
    AnalysisFun { name: Name },
    Var(VarId),
}

/*
use BodyDiagnostic::*;
impl_display! {
    match BodyDiagnostic {
        ExpectedPort{..} => "";
        _ => "";
    }
}
*/

impl BodyDiagnostic {
    pub fn validate_and_collect(db: &dyn HirTyDB, def: DefWithBodyId) -> Vec<BodyDiagnostic> {
        let body = &db.body(def);
        let infer = &db.inference_result(def);
        let ctx = match def {
            DefWithBodyId::ModuleId { initial: false, .. } => BodyCtx::AnalogBlock,
            DefWithBodyId::ModuleId { initial: true, .. } => BodyCtx::AnalogInitialBlock,
            DefWithBodyId::FunctionId(_) => BodyCtx::Function,
            _ => BodyCtx::Const,
        };
        let mut validator = BodyValidator {
            db,
            owner: def,
            body,
            infer,
            ctx,
            diagnostics: Vec::new(),
            non_const_dominator: Box::default(),
            non_trivial_branches: HashSet::default(),
            trivial_probes: HashMap::default(),
        };

        for stmt in &body.entry_stmts {
            validator.validate_stmt(*stmt)
        }

        for (branch, exprs) in validator.trivial_probes {
            for (stmt, expr) in exprs {
                validator.diagnostics.push(BodyDiagnostic::TrivialBranchAccess {
                    branch,
                    expr,
                    stmt,
                })
            }
        }

        validator.diagnostics
    }
}
