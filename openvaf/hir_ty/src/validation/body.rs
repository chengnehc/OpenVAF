//use stdx::impl_display;

use ahash::{HashMap, HashSet};
use hir_def::{
    BranchId, BuiltIn, DefWithBodyId, ExprId, FunctionArgLoc, NatureId, NodeId, ParamId, StmtId,
    VarId,
};
use syntax::name::Name;

use crate::db::HirTyDB;
use crate::inference::BranchWrite;

use super::{BodyContext, BodyValidator};

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
    IncompatibleImplicitBranch {
        access_expr: ExprId,
        node1: NodeId,
        node2: NodeId,
    },

    /* Scope rule violation */
    IllegalContribute {
        stmt: StmtId,
        ctxt: BodyContext,
    },
    IllegalParamAccess {
        def: ParamId,
        expr: ExprId,
        param: ParamId,
    },
    IllegalCtxtAccess {
        kind: IllegalCtxtAccessKind,
        ctxt: BodyContext,
        expr: ExprId,
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

    /* Nature access */
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
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum IllegalCtxtAccessKind {
    NatureAccess,
    AnalogOperator { name: Name, is_standard: bool, non_const_dominator: Box<[ExprId]> },
    AnalysisFun { name: Name },
    Var(VarId),
}

impl BodyDiagnostic {
    pub fn validate_and_collect(db: &dyn HirTyDB, def: DefWithBodyId) -> Vec<BodyDiagnostic> {
        let body = &db.body(def);
        let infer = &db.inference_result(def);
        let body_ctxt = match def {
            DefWithBodyId::ModuleId { initial: false, .. } => BodyContext::AnalogBlock,
            DefWithBodyId::ModuleId { initial: true, .. } => BodyContext::AnalogInitialBlock,
            DefWithBodyId::FunctionId(_) => BodyContext::Function,
            _ => BodyContext::Const,
        };
        let mut validator = BodyValidator {
            db,
            owner: def,
            body,
            infer,
            ctxt: body_ctxt,
            non_const_dominator: Box::default(),
            non_trivial_branches: HashSet::default(),
            trivial_probes: HashMap::default(),
            diagnostics: Vec::new(),
        };
        for stmt in &body.entry_stmts {
            validator.validate_stmt(*stmt)
        }
        for (branch, exprs) in validator.trivial_probes {
            for (stmt, expr) in exprs {
                let diag = BodyDiagnostic::TrivialBranchAccess { branch, expr, stmt };
                validator.diagnostics.push(diag);
            }
        }

        validator.diagnostics
    }
}
