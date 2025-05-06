use std::{iter, mem};
use stdx::impl_display;

use ahash::{HashMap, HashSet};
use basedb::FileId;
use hir_def::{
    body::Body,
    nameres::{DefMap, PathResolveError, ScopeItemDef},
    AliasParamId, Branch, BranchId, BuiltIn, DefWithBodyId, DisciplineId, Expr, ExprId,
    FunctionArgLoc, ItemLoc, ItemTree, Literal, Lookup, ModuleId, ModuleLoc, NatureId, NodeId,
    NodeTypeDecl, Path, Scope, Stmt, StmtId,
};
use syntax::{
    ast::{ArgListOwner, AssignOp},
    name::AsIdent,
    {AstNode, SyntaxNodePtr},
};
use typed_index_collections::TiSlice;

use crate::builtin::{
    ABSDELAY_MAX, DDT_TOL, IDT_IC_ASSERT_TOL, NATURE_ACCESS_BRANCH, NATURE_ACCESS_NODES,
    NATURE_ACCESS_NODE_GND, NATURE_ACCESS_PORT_FLOW, NOISE_TABLE_INLINE, NOISE_TABLE_INLINE_NAME,
    TRANSITION_DELAY_RISET_FALLT_TOL,
};
use crate::inference::{BranchWrite, ResolvedFun};
use crate::lower::BranchKind;
use crate::types::{Signature, Ty};
use crate::{HirTyDB, Inference};

mod body;
mod diagnostics;
mod types;

use body::IllegalCtxtAccessKind;
use types::DuplicateItem;

pub use body::BodyDiagnostic;
pub use diagnostics::{BodyDiagnosticWrapped, TypeDiagnosticWrapped};
pub use types::TypeDiagnostic;

struct BodyValidator<'a> {
    db: &'a dyn HirTyDB,
    infer: &'a Inference,
    owner: DefWithBodyId,
    body: &'a Body,
    ctxt: BodyContext,
    non_const_dominator: Box<[ExprId]>,
    non_trivial_branches: HashSet<BranchWrite>,
    trivial_probes: HashMap<BranchWrite, Vec<(StmtId, ExprId)>>,
    diagnostics: Vec<BodyDiagnostic>,
}

impl BodyValidator<'_> {
    fn validate_stmt(&mut self, stmt: StmtId) {
        match self.body.stmts[stmt] {
            Stmt::Missing | Stmt::Empty => (),
            Stmt::Expr(e) => self.validate_expr(e, stmt),
            Stmt::Block { ref body } => body.iter().for_each(|stmt| self.validate_stmt(*stmt)),
            Stmt::Assignment { dst, val, op_kind } => {
                self.validate_expr(val, stmt);
                if op_kind == AssignOp::Contribute && !self.ctxt.contribute_allowed() {
                    self.diagnostics
                        .push(BodyDiagnostic::IllegalContribute { stmt, ctxt: self.ctxt })
                }
                // avoid duplicate errors
                else if self.infer.assignment_destination.contains_key(&stmt) {
                    self.validate_assignment_dst(dst, stmt);
                }
            }
            Stmt::EventControl { body, .. } => {
                let old = mem::replace(&mut self.ctxt, BodyContext::EventControl);
                self.validate_stmt(body);
                self.ctxt = old;
            }
            Stmt::If { cond, .. }
            | Stmt::ForLoop { cond, .. }
            | Stmt::WhileLoop { cond, .. }
            | Stmt::Case { discr: cond, .. } => {
                self.validate_condition(cond, stmt, |s| {
                    s.body.stmts[stmt].walk_child_stmts(|stmt| s.validate_stmt(stmt))
                });
            }
        }
    }

    fn validate_expr(&mut self, expr: ExprId, stmt: StmtId) {
        ExprValidator { parent: self, is_write: false, stmt, sink: None }.validate_expr(expr)
    }

    fn validate_assignment_dst(&mut self, expr: ExprId, stmt: StmtId) {
        ExprValidator { parent: self, is_write: true, stmt, sink: None }.validate_expr(expr)
    }

    fn validate_condition(
        &mut self,
        cond: ExprId,
        stmt: StmtId,
        f: impl FnOnce(&mut Self),
    ) -> Option<Box<[ExprId]>> {
        if self.ctxt == BodyContext::AnalogBlock || self.ctxt == BodyContext::Conditional {
            let mut non_const_access = Vec::new();
            ExprValidator {
                parent: self,
                is_write: false,
                stmt,
                sink: Some(&mut non_const_access),
            }
            .validate_expr(cond);
            if !non_const_access.is_empty() {
                let non_const_dominator = mem::replace(
                    &mut self.non_const_dominator,
                    non_const_access.into_boxed_slice(),
                );
                let ctxt = mem::replace(&mut self.ctxt, BodyContext::Conditional);
                f(self);
                self.ctxt = ctxt;

                return Some(mem::replace(&mut self.non_const_dominator, non_const_dominator));
            }
        } else {
            self.validate_expr(cond, stmt);
        }
        f(self);

        None
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum BodyContext {
    AnalogBlock,
    AnalogInitialBlock,
    Conditional, // if, case, or ?:
    EventControl,
    Function,
    Const,
    ConstOrAnalysis,
}

// analog initial block shall not contain the following statements:
// — statements with access functions or analog operators;
// — contribution statements;
// — event control statements.
// This is similar to the restrictions on the statements in analog functions.

impl BodyContext {
    fn nature_access_allowed(self) -> bool {
        matches!(self, Self::AnalogBlock | Self::Conditional | Self::EventControl)
    }
    fn contribute_allowed(self) -> bool {
        matches!(self, Self::AnalogBlock | Self::Conditional)
    }
    /// [LRM 4.5.15] Restrictions on analog operators
    ///
    /// Analog operators can only be used inside an analog block; they can not be
    /// used inside an initial or always block, or inside a user-defined function.
    fn analog_operator_allowed(self) -> bool {
        matches!(self, Self::AnalogBlock)
    }
    // FIXME(JW) likely wrong
    fn analysis_fun_allowed(self) -> bool {
        !matches!(self, Self::Const)
    }
    fn var_ref_allowed(self) -> bool {
        !matches!(self, Self::Const | Self::ConstOrAnalysis)
    }
}

impl_display! {
    match BodyContext{
       BodyContext::AnalogBlock => "analog block";
       BodyContext::AnalogInitialBlock => "analog initial block";
       BodyContext::Conditional => "conditions";
       BodyContext::EventControl => "events";
       BodyContext::Function => "analog functions";
       BodyContext::Const => "constants";
       BodyContext::ConstOrAnalysis => "constant or analysis";
    }
}

struct ExprValidator<'a, 'b> {
    parent: &'a mut BodyValidator<'b>,
    stmt: StmtId,
    is_write: bool,
    sink: Option<&'a mut Vec<ExprId>>,
}

impl ExprValidator<'_, '_> {
    fn validate_expr(&mut self, expr: ExprId) {
        match self.parent.body.exprs[expr] {
            Expr::Path { port: false, .. } => {
                match self.parent.infer.expr_types[expr] {
                    Ty::Var(_, var) => {
                        self.check_access(
                            |_| IllegalCtxtAccessKind::Var(var),
                            expr,
                            self.parent.ctxt.var_ref_allowed(),
                        );
                    }
                    Ty::FunctionVar { arg: Some(arg), fun, .. } => {
                        let is_output = self.parent.db.function_data(fun).args[arg].is_output;
                        if self.is_write && !is_output {
                            self.report(BodyDiagnostic::WriteToInputArg {
                                expr,
                                arg: FunctionArgLoc { fun, id: arg },
                            })
                        }
                    }
                    Ty::Param(_, param) => {
                        if let DefWithBodyId::ParamId(def) = self.parent.owner {
                            if def.lookup(self.parent.db.upcast()).id
                                < param.lookup(self.parent.db.upcast()).id
                            {
                                self.report(BodyDiagnostic::IllegalParamAccess { def, expr, param })
                            }
                        }
                    }
                    _ => (),
                };
                return;
            }
            Expr::Call { ref fun, ref args, .. } => {
                match self.parent.infer.resolved_calls.get(&expr) {
                    Some(ResolvedFun::BuiltIn(builtin)) => {
                        let signature = self.parent.infer.resolved_signatures.get(&expr);
                        self.validate_builtin(fun, expr, args, *builtin, signature.cloned());
                        return;
                    }
                    Some(ResolvedFun::InvalidNatureAccess(nature)) => {
                        self.validate_nature_access(*nature, expr, args);
                        return;
                    }
                    _ => (),
                }
            }
            Expr::Select { cond, then_val, else_val } => {
                if let Some(non_const_dominators) =
                    self.parent.validate_condition(cond, self.stmt, |s| {
                        let mut validator = ExprValidator {
                            parent: s,
                            sink: self.sink.as_deref_mut(),
                            is_write: false,
                            stmt: self.stmt,
                        };
                        validator.validate_expr(then_val);
                        validator.validate_expr(else_val);
                    })
                {
                    if let Some(sink) = &mut self.sink {
                        sink.extend(non_const_dominators.to_vec())
                    }
                }
            }

            _ => (),
        }

        self.parent.body.exprs[expr].walk_child_exprs(|child| self.validate_expr(child))
    }

    fn validate_nature_access(
        &mut self,
        access_nature: NatureId,
        access_expr: ExprId,
        args: &[ExprId],
    ) {
        match self.parent.infer.resolved_signatures.get(&access_expr).copied() {
            Some(NATURE_ACCESS_BRANCH) => {
                let branch = self.parent.infer.expr_types[args[0]].unwrap_branch();
                if let Some(branch_info) = self.parent.db.branch_info(branch) {
                    self.report_illegal_nature_access(
                        self.parent.db.branch_data(branch).name.to_string(),
                        branch_info.discipline,
                        Some(access_nature),
                        access_expr,
                    )
                }
            }
            Some(NATURE_ACCESS_NODE_GND) => {
                let node = self.parent.infer.expr_types[args[0]].unwrap_node();
                if let Some(discipline) = self.parent.db.node_discipline(node) {
                    let node = self.parent.db.node_data(node);
                    self.report_illegal_nature_access(
                        format!("({})", node.name),
                        discipline,
                        Some(access_nature),
                        access_expr,
                    )
                }
            }
            Some(NATURE_ACCESS_NODES) => {
                let node1 = self.parent.infer.expr_types[args[0]].unwrap_node();
                let node2 = self.parent.infer.expr_types[args[1]].unwrap_node();
                if let Some(discipline1) = self.parent.db.node_discipline(node1) {
                    if let Some(discipline2) = self.parent.db.node_discipline(node2) {
                        let discipline2 = self.parent.db.discipline_info(discipline2);
                        if discipline2.compatible(discipline1, self.parent.db) {
                            let node1 = self.parent.db.node_data(node1);
                            let node2 = self.parent.db.node_data(node2);
                            self.report_illegal_nature_access(
                                format!("({}, {})", node1.name, node2.name),
                                discipline1,
                                Some(access_nature),
                                access_expr,
                            )
                        } else {
                            self.report(BodyDiagnostic::IncompatibleImplicitBranch {
                                access_expr,
                                node1,
                                node2,
                            })
                        }
                    }
                }
            }
            Some(NATURE_ACCESS_PORT_FLOW) => {
                let node = self.parent.infer.expr_types[args[0]].unwrap_port_flow();
                if let Some(discipline) = self.parent.db.node_discipline(node) {
                    let node = self.parent.db.node_data(node);
                    self.report_illegal_nature_access(
                        format!("(<{}>)", node.name),
                        discipline,
                        Some(access_nature),
                        access_expr,
                    )
                }
            }
            _ => (),
        };
    }

    fn validate_builtin(
        &mut self,
        path: &Option<Path>,
        expr: ExprId,
        mut args: &[ExprId],
        builtin: BuiltIn,
        signature: Option<Signature>,
    ) {
        match builtin {
            _ if builtin.is_unsupported() => self
                .parent
                .diagnostics
                .push(BodyDiagnostic::UnsupportedFunction { expr, func: builtin }),

            BuiltIn::potential | BuiltIn::flow => self.check_access(
                |_| IllegalCtxtAccessKind::NatureAccess,
                expr,
                self.parent.ctxt.nature_access_allowed(),
            ),

            _ if builtin.is_analog_operator() && builtin != BuiltIn::ddx
                || builtin.is_analog_operator_sysfun() =>
            {
                // let non_const_dominator = if self.cond_diagnostic_sink.is_none() {
                // self.parent.non_const_dominator.clone()
                // } else {
                // vec![].into_boxed_slice()
                // };

                self.check_access(
                    |validator| IllegalCtxtAccessKind::AnalogOperator {
                        name: path.as_ref().and_then(|p| p.as_ident()).unwrap(),
                        is_standard: builtin.is_analog_operator(),
                        non_const_dominator: validator.parent.non_const_dominator.clone(),
                    },
                    expr,
                    self.parent.ctxt.analog_operator_allowed(),
                )
            }

            _ if builtin.is_analysis_fun() && !self.parent.ctxt.analysis_fun_allowed() => self
                .report_illegal_access(
                    IllegalCtxtAccessKind::AnalysisFun {
                        name: path.as_ref().and_then(|p| p.as_ident()).unwrap(),
                    },
                    expr,
                ),

            _ => (),
        }

        match (builtin, signature) {
            (BuiltIn::potential | BuiltIn::flow, Some(NATURE_ACCESS_NODES)) => {
                let hi = self.parent.infer.expr_types[args[0]].unwrap_node();
                let lo = self.parent.infer.expr_types[args[1]].unwrap_node(); // JW: should be args[1]
                let branch = if hi >= lo {
                    BranchWrite::Unnamed { hi, lo: Some(lo) }
                } else {
                    BranchWrite::Unnamed { hi: lo, lo: Some(hi) }
                };
                self.lint_trivial_branch(branch, builtin, expr);
                if let Some(discipline) = self.validate_implicit_branch(expr, hi, lo) {
                    self.validate_flow_or_pot(expr, builtin, discipline)
                }
            }

            (BuiltIn::potential | BuiltIn::flow, Some(NATURE_ACCESS_NODE_GND)) => {
                let node = self.parent.infer.expr_types[args[0]].unwrap_node();
                if let Some(discipline) = self.parent.db.node_discipline(node) {
                    self.lint_trivial_branch(
                        BranchWrite::Unnamed { hi: node, lo: None },
                        builtin,
                        expr,
                    );
                    self.validate_flow_or_pot(expr, builtin, discipline)
                }
            }

            (BuiltIn::flow, Some(NATURE_ACCESS_PORT_FLOW)) => {
                let node = self.parent.infer.expr_types[args[0]].unwrap_port_flow();
                let node_data = self.parent.db.node_data(node);
                if !(node_data.is_input | node_data.is_output) {
                    self.report(BodyDiagnostic::ExpectedPort { node, expr })
                }

                if let Some(discipline) = self.parent.db.node_discipline(node) {
                    self.validate_flow_or_pot(expr, BuiltIn::flow, discipline)
                }
            }

            (BuiltIn::potential, Some(NATURE_ACCESS_PORT_FLOW)) => {
                self.report(BodyDiagnostic::PotentialOfPortFlow { expr, branch: None })
            }

            (BuiltIn::potential | BuiltIn::flow, Some(NATURE_ACCESS_BRANCH)) => {
                let branch = self.parent.infer.expr_types[args[0]].unwrap_branch();

                if let Some(branch_info) = self.parent.db.branch_info(branch) {
                    match branch_info.kind {
                        BranchKind::PortFlow(_) => {
                            if builtin == BuiltIn::potential {
                                self.report(BodyDiagnostic::PotentialOfPortFlow {
                                    expr,
                                    branch: Some(branch),
                                })
                            } else if !self.is_write {
                                self.validate_flow_or_pot(
                                    expr,
                                    BuiltIn::flow,
                                    branch_info.discipline,
                                )
                            }
                        }
                        BranchKind::NodeGnd(node) => {
                            self.lint_trivial_branch(
                                BranchWrite::Unnamed { hi: node, lo: None },
                                builtin,
                                expr,
                            );
                            self.validate_flow_or_pot(expr, builtin, branch_info.discipline)
                        }
                        BranchKind::Nodes(hi, lo) => {
                            let branch = if hi >= lo {
                                BranchWrite::Unnamed { hi, lo: Some(lo) }
                            } else {
                                BranchWrite::Unnamed { hi: lo, lo: Some(hi) }
                            };
                            self.lint_trivial_branch(branch, builtin, expr);
                            self.validate_flow_or_pot(expr, builtin, branch_info.discipline)
                        }
                    }
                }
            }

            (BuiltIn::port_connected, _) => {
                let node = self.parent.infer.expr_types[args[0]].unwrap_node();
                let node_data = self.parent.db.node_data(node);
                if !(node_data.is_input | node_data.is_output) {
                    self.report(BodyDiagnostic::ExpectedPort { node, expr })
                }
            }

            (
                BuiltIn::noise_table | BuiltIn::noise_table_log,
                Some(NOISE_TABLE_INLINE | NOISE_TABLE_INLINE_NAME),
            ) => self.validate_const_expr(args[0]),

            (func @ (BuiltIn::simparam | BuiltIn::simparam_str), _) => {
                if self.parent.ctxt == BodyContext::Const {
                    let known = if let Expr::Literal(Literal::String(name)) =
                        &self.parent.body.exprs[args[0]]
                    {
                        matches!(
                            (func, &**name),
                            (
                                BuiltIn::simparam,
                                "minr"
                                    | "rthresh"
                                    | "imax"
                                    | "imelt"
                                    | "scale"
                                    | "shrink"
                                    | "simulatorSubversion"
                                    | "simulatorVersion"
                                    | "tnom"
                            ) | (BuiltIn::simparam_str, "cwd" | "module" | "instance" | "path")
                        )
                    } else {
                        false
                    };

                    self.report(BodyDiagnostic::ConstSimparam { known, expr, stmt: self.stmt });
                }
            }

            (BuiltIn::absdelay, Some(ABSDELAY_MAX))
            | (BuiltIn::transition, Some(TRANSITION_DELAY_RISET_FALLT_TOL))
            | (BuiltIn::ddt, Some(DDT_TOL))
            | (BuiltIn::idt | BuiltIn::idtmod, Some(IDT_IC_ASSERT_TOL)) => {
                if let [other_args @ .., const_expr] = args {
                    args = other_args; // Do not type check const expr twice
                    self.validate_const_expr(*const_expr);
                };
            }

            /* JW: not supported
            (
                BuiltIn::laplace_nd
                | BuiltIn::laplace_np
                | BuiltIn::laplace_zp
                | BuiltIn::laplace_zd
                | BuiltIn::zi_nd
                | BuiltIn::zi_np
                | BuiltIn::zi_zd
                | BuiltIn::zi_zp,
                Some(_),
            ) => {
                if let [_expr, const_args @ ..] = args {
                    args = &args[..1];
                    for arg in const_args {
                        self.validate_const_expr(*arg)
                    }
                }
            }
            */
            _ => (),
        }

        for arg in args {
            self.validate_expr(*arg)
        }
    }

    fn validate_const_expr(&mut self, expr: ExprId) {
        let old = mem::replace(&mut self.parent.ctxt, BodyContext::Const);
        let sink = self.sink.take();
        self.validate_expr(expr);
        self.sink = sink;
        self.parent.ctxt = old;
    }

    fn lint_trivial_branch(&mut self, branch: BranchWrite, call: BuiltIn, expr: ExprId) {
        let is_flow = call == BuiltIn::flow;
        if self.is_write {
            self.parent.non_trivial_branches.insert(branch);
            self.parent.trivial_probes.remove(&branch);
        } else if is_flow && !self.parent.non_trivial_branches.contains(&branch) {
            self.parent.trivial_probes.entry(branch).or_default().push((self.stmt, expr))
        }
    }

    fn validate_flow_or_pot(&mut self, expr: ExprId, call: BuiltIn, discipline: DisciplineId) {
        let is_pot = call == BuiltIn::potential;
        let discipline_ = self.parent.db.discipline_info(discipline);
        if discipline_.potential.is_none() && is_pot || discipline_.flow.is_none() && !is_pot {
            self.report(BodyDiagnostic::IllegalNatureAccess { is_pot, access_expr: expr })
        }
    }

    fn validate_implicit_branch(
        &mut self,
        expr: ExprId,
        node1: NodeId,
        node2: NodeId,
    ) -> Option<DisciplineId> {
        if let Some(discipline1) = self.parent.db.node_discipline(node1) {
            if let Some(discipline2) = self.parent.db.node_discipline(node2) {
                let discipline2 = self.parent.db.discipline_info(discipline2);
                if !discipline2.compatible(discipline1, self.parent.db) {
                    self.report(BodyDiagnostic::IncompatibleImplicitBranch {
                        access_expr: expr,
                        node1,
                        node2,
                    });
                } else {
                    return Some(discipline1);
                }
            }
        }

        None
    }

    fn check_access(
        &mut self,
        kind: impl FnOnce(&Self) -> IllegalCtxtAccessKind,
        expr: ExprId,
        allowed: bool,
    ) {
        if let Some(sink) = &mut self.sink {
            sink.push(expr)
        }
        if !allowed {
            self.report_illegal_access(kind(self), expr)
        }
    }

    fn report_illegal_access(&mut self, kind: IllegalCtxtAccessKind, expr: ExprId) {
        let err = BodyDiagnostic::IllegalCtxtAccess { kind, ctxt: self.parent.ctxt, expr };
        self.report(err);
    }

    fn report_illegal_nature_access(
        &mut self,
        branch: String,
        discipline: DisciplineId,
        access_nature: Option<NatureId>,
        access_expr: ExprId,
    ) {
        let db = self.parent.db;
        let discipline = db.discipline_info(discipline);

        let nature_info = |nature: NatureId| {
            let nature = nature.lookup(db.upcast());
            let nature = &nature.item_tree(db.upcast())[nature.id];
            Some((nature.name.clone(), nature.access.clone()?.0))
        };
        let pot = discipline.potential.and_then(nature_info);
        let flow = discipline.flow.and_then(nature_info);
        self.parent.diagnostics.push(BodyDiagnostic::IncompatibleNatureAccess {
            candidates: [pot, flow],
            access_nature,
            access_expr,
            branch,
        })
    }

    fn report(&mut self, diagnostic: BodyDiagnostic) {
        self.parent.diagnostics.push(diagnostic)
    }
}

struct TypeValidator<'a> {
    db: &'a dyn HirTyDB,
    tree: &'a ItemTree,
    def_map: &'a DefMap,
    root_file: FileId,
    diagnostics: &'a mut Vec<TypeDiagnostic>,
}

impl TypeValidator<'_> {
    fn validate(&mut self) {
        let root = &self.def_map[self.def_map.root_scope()];
        for def in root.declarations.values() {
            match *def {
                ScopeItemDef::NatureId(nature) => self.verify_nature(nature),
                ScopeItemDef::DisciplineId(discipline) => self.verify_discipline(discipline),
                ScopeItemDef::ModuleId(module) => self.verify_module(module),
                _ => (),
            }
        }
    }

    fn verify_nature(&mut self, nature: NatureId) {
        // let info = self.db.nature_info(nature);
        let data = self.db.nature_data(nature);
        self.verify_unique_attr(&data.attrs, nature, TypeDiagnostic::DuplicateNatureAttr);
    }

    // TODO check natures/discipline (~dspom/OpenVAF#1)
    fn verify_discipline(&mut self, discipline: DisciplineId) {
        // let info = self.db.discipline_info(discipline);
        let data = self.db.discipline_data(discipline);
        self.verify_unique_attr(&data.attrs, discipline, TypeDiagnostic::DuplicateDisciplineAttr);
    }

    fn verify_unique_attr<Attr: From<usize> + PartialEq, Def: Copy>(
        &mut self,
        attrs: &TiSlice<Attr, impl PartialEq>,
        def: Def,
        wrap_err: impl Fn(DuplicateItem<Attr, Def>) -> TypeDiagnostic,
    ) {
        // This is quadratic (actually its n(n+1)/2). But disciplines and nature usually only have very few (below 5)
        // attributes so this is probably faster than allocating a HashMap. If this ever becomes a
        // problem just use a HashMap instead
        for (id, attr) in attrs.iter_enumerated() {
            let mut duplicates =
                attrs.iter_enumerated().filter_map(
                    |it| {
                        if it.1 == attr {
                            Some(it.0)
                        } else {
                            None
                        }
                    },
                );
            if duplicates.next().unwrap() != id {
                continue;
            }
            let duplicates: Vec<_> = duplicates.collect();
            if !duplicates.is_empty() {
                let err = DuplicateItem { src: def, first: id, subsequent: duplicates };
                self.report(wrap_err(err))
            }
        }
    }

    fn verify_module(&mut self, module: ModuleId) {
        let loc = module.lookup(self.db.upcast());
        let scope = loc.scope.local_id;
        for item in self.def_map[scope].declarations.values() {
            match item {
                ScopeItemDef::NodeId(node) => self.verify_node(*node, loc),
                ScopeItemDef::BranchId(branch) => self.verify_branch(*branch),
                ScopeItemDef::AliasParamId(alias) => self.verify_alias(*alias),
                _ => (),
            }
        }
    }

    fn verify_node(&mut self, node: NodeId, module: ModuleLoc) {
        let loc = node.lookup(self.db.upcast());
        let node_ = &self.tree[module.id].nodes[loc.id];
        if node_.decls.is_empty() {
            self.report(TypeDiagnostic::PortWithoutDirection {
                decl: node_.ast_id,
                name: node_.name.clone(),
            });
            self.report(TypeDiagnostic::NodeWithoutDiscipline {
                decl: node_.ast_id,
                name: node_.name.clone(),
            });
            return; // Do not print other diagnostics here would just lead to duplications
        }
        let mut directions = node_.decls.iter().filter_map(|decl| {
            if let NodeTypeDecl::Port(p) = decl {
                Some(self.tree[*p].ast_id)
            } else {
                None
            }
        });
        if let Some(first) = directions.next() {
            let duplicates: Vec<_> = directions.collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleDirections(DuplicateItem {
                    src: node,
                    first,
                    subsequent: duplicates,
                }))
            }
        } else if node_.decls[0].ast_id(self.tree) != node_.ast_id {
            self.report(TypeDiagnostic::PortWithoutDirection {
                decl: node_.ast_id,
                name: node_.name.clone(),
            })
        }

        let mut disciplines = node_
            .decls
            .iter()
            .filter_map(|it| it.discipline(self.tree).as_ref().map(|discipline| (it, discipline)));

        if let Some((decl, discipline)) = disciplines.next() {
            for (decl, discipline) in iter::once((decl, discipline)).chain(disciplines.clone()) {
                if let Err(err) = self
                    .def_map
                    .resolve_item_name::<DisciplineId>(self.def_map.root_scope(), discipline)
                {
                    self.report(TypeDiagnostic::PathError {
                        err,
                        src: SyntaxNodePtr::new(
                            decl.discipline_source(self.db.upcast(), self.root_file)
                                .unwrap()
                                .syntax(),
                        ),
                    })
                }
            }

            let duplicates: Vec<_> = disciplines.map(|(decl, _)| decl.ast_id(self.tree)).collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleDisciplines(DuplicateItem {
                    src: node,
                    first: decl.ast_id(self.tree),
                    subsequent: duplicates,
                }))
            }
        } else {
            self.report(TypeDiagnostic::NodeWithoutDiscipline {
                decl: node_.ast_id,
                name: node_.name.clone(),
            });
        }

        let mut gnd_declarations = node_.decls.iter().filter(|it| it.is_gnd(self.tree));

        if let Some(first) = gnd_declarations.next() {
            let duplicates: Vec<_> = gnd_declarations.map(|it| it.ast_id(self.tree)).collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleDisciplines(DuplicateItem {
                    src: node,
                    first: first.ast_id(self.tree),
                    subsequent: duplicates,
                }))
            }
        }

        // TODO(JW): multiple grounds
    }

    fn verify_branch(&mut self, branch_: BranchId) {
        let branch_data = self.db.branch_data(branch_);
        let kind = &branch_data.kind;
        let branch = branch_.lookup(self.db.upcast());
        let scope = branch.scope;
        match kind {
            hir_def::BranchKind::Missing => (),
            hir_def::BranchKind::PortFlow(port) => {
                if let Some(node) = self.resolve_node(port, scope, &branch) {
                    let node_ = self.db.node_data(node);
                    if !node_.is_input && !node_.is_output {
                        let src = branch.ast_id(self.db.upcast()).into();
                        self.report(TypeDiagnostic::ExpectedPort { node, src });
                    }
                }
            }
            hir_def::BranchKind::NodeGnd(node) => {
                self.resolve_node(node, scope, &branch);
            }
            hir_def::BranchKind::Nodes(node1, node2) => {
                let node1 = self.resolve_node(node1, scope, &branch);
                let node2 = self.resolve_node(node2, scope, &branch);
                let (Some(node1), Some(node2)) = (node1, node2) else { return };

                let discipline1 = self.db.node_discipline(node1);
                let discipline2 = self.db.node_discipline(node2);
                // fast path
                if discipline1 == discipline2 {
                    return;
                }
                let (Some(d1), Some(d2)) = (discipline1, discipline2) else { return };
                if !self.db.discipline_info(d1).compatible(d2, self.db) {
                    self.report(TypeDiagnostic::IncompatibleBranch {
                        branch: branch_,
                        node1,
                        node2,
                    })
                }
            }
        };
    }

    fn resolve_node(
        &mut self,
        node: &Path,
        scope: Scope,
        branch: &ItemLoc<Branch>,
    ) -> Option<NodeId> {
        let node = scope.resolve_item_path::<NodeId>(self.db.upcast(), node);
        match node {
            Ok(node) => Some(node),
            Err(err) => {
                let src = SyntaxNodePtr::new(
                    branch
                        .source(self.db.upcast())
                        .arg_list()
                        .unwrap()
                        .args()
                        .next()
                        .unwrap()
                        .syntax(),
                );
                self.report(TypeDiagnostic::PathError { err, src });
                None
            }
        }
    }

    fn verify_alias(&mut self, alias: AliasParamId) {
        if self.db.resolve_alias(alias).is_none() {
            let loc = alias.lookup(self.db.upcast());
            let data = self.db.aliasparam_data(alias);
            if let Some(path) = data.src.as_ref() {
                match loc.scope.resolve_path(self.db.upcast(), path) {
                    // TODO: better errors for cycles
                    Ok(found) => {
                        let src = SyntaxNodePtr::new(
                            loc.source(self.db.upcast()).src().unwrap().syntax(),
                        );
                        self.report(TypeDiagnostic::PathError {
                            err: PathResolveError::ExpectedItemKind {
                                name: path.segments.last().unwrap().clone(),
                                expected: "parameter",
                                found,
                            },
                            src,
                        })
                    }
                    Err(err) => {
                        let src = SyntaxNodePtr::new(
                            loc.source(self.db.upcast()).src().unwrap().syntax(),
                        );
                        self.report(TypeDiagnostic::PathError { err, src })
                    }
                }
            }
        }
    }

    fn report(&mut self, diag: impl Into<TypeDiagnostic>) {
        self.diagnostics.push(diag.into())
    }
}
