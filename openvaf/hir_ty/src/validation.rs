use stdx::impl_display;

use ahash::{HashMap, HashSet};
use basedb::FileId;
use hir_def::{
    body::{Body, ExprId, StmtId},
    nameres::DefMap,
    DefWithBodyId, ItemTree, Lookup,
};

use crate::db::HirTyDB;
use crate::inference::{BranchWrite, Inference, ResolvedFun};

mod body;
mod ty;

mod diagnostics;
pub use diagnostics::{
    BodyDiagnostic, BodyDiagnosticWrapped, TypeDiagnostic, TypeDiagnosticWrapped,
};

pub struct TypeValidator<'a> {
    db: &'a dyn HirTyDB,
    root_file: FileId,
    item_tree: &'a ItemTree,
    def_map: &'a DefMap,
    diagnostics: Vec<TypeDiagnostic>,
}

impl TypeDiagnostic {
    pub fn validate_and_collect(db: &dyn HirTyDB, root_file: FileId) -> Vec<TypeDiagnostic> {
        let item_tree = &db.item_tree(root_file);
        let def_map = &db.root_def_map(root_file);
        TypeValidator { db, root_file, item_tree, def_map, diagnostics: Vec::new() }.validate()
    }
}

pub struct BodyValidator<'a> {
    def: DefWithBodyId,
    // reads
    db: &'a dyn HirTyDB,
    infer: &'a Inference,
    body: &'a Body,
    // states
    ctxt: BodyContext,
    // for condition validation
    non_const_dominator: Box<[ExprId]>,
    // for trivial branch linting
    non_trivial_branches: HashSet<BranchWrite>,
    trivial_probes: HashMap<BranchWrite, Vec<(StmtId, ExprId)>>,
    // output
    diagnostics: Vec<BodyDiagnostic>,
}

// LRM 4.5.15
// Analog operators can only be used inside an analog block; they can not be
// used inside an initial or always block, or inside a user-defined function.
//
// Analog operators shall not be used inside conditional (if, case, or ?:)
// statements unless the conditional expression controlling the statement
// consists of terms which can not change their value during simulation.

// LRM 4.7.1
// Analog functions
// - shall not use access functions;
// - shall not use analog filter functions (analog operators);
// - shall not use contribution statements or event control statements;

// LRM 5.2.1
// Analog initial block shall not contain the following statements:
// - statements with access functions or analog operators;
// - contribution statements;
// - event control statements.

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum BodyContext {
    AnalogBlock,
    AnalogInitialBlock,
    Function,
    Conditional, // if, case, or ?:
    Const,
    // not fully implemented
    EventControl,
    ConstOrAnalysis,
}

impl_display! {
    match BodyContext{
       BodyContext::AnalogBlock => "analog block";
       BodyContext::AnalogInitialBlock => "analog initial block";
       BodyContext::Function => "analog functions";
       BodyContext::Conditional => "conditions";
       BodyContext::Const => "constants";
       BodyContext::EventControl => "events";
       BodyContext::ConstOrAnalysis => "constant or analysis";
    }
}

impl BodyContext {
    fn nature_access_allowed(self) -> bool {
        matches!(self, Self::AnalogBlock | Self::Conditional | Self::EventControl)
    }
    fn contribute_allowed(self) -> bool {
        matches!(self, Self::AnalogBlock | Self::Conditional)
    }
    fn analog_operator_allowed(self) -> bool {
        matches!(self, Self::AnalogBlock)
    }
    fn analysis_fun_allowed(self) -> bool {
        !matches!(self, Self::Const)
    }
    fn var_ref_allowed(self) -> bool {
        !matches!(self, Self::Const | Self::ConstOrAnalysis)
    }
}

/// Body expression validator
struct ExprValidator<'a, 'b> {
    parent: &'a mut BodyValidator<'b>,
    sink: Option<&'a mut Vec<ExprId>>,
    stmt: StmtId,
    is_write: bool, // Is this expression a assigment dst?
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
        BodyValidator {
            def,
            db,
            body,
            infer,
            ctxt: body_ctxt,
            non_const_dominator: Box::default(),
            non_trivial_branches: HashSet::default(),
            trivial_probes: HashMap::default(),
            diagnostics: Vec::new(),
        }
        .validate()
    }
}
