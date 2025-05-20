use std::mem;

use hir_def::{
    body::{Expr, ExprId, Literal, Stmt, StmtId},
    BuiltIn, DisciplineId, FunctionArgLoc, NatureId, NodeId,
};
use syntax::{ast, AsIdent, Name};

use crate::builtin::{
    ABSDELAY_MAX, DDT_TOL, IDT_IC_ASSERT_TOL, NATURE_ACCESS_BRANCH, NATURE_ACCESS_NODES,
    NATURE_ACCESS_NODE_GND, NATURE_ACCESS_PORT_FLOW, NOISE_TABLE_INLINE, NOISE_TABLE_INLINE_NAME,
    TRANSITION_DELAY_RISET_FALLT_TOL,
};
use crate::lower::BranchKind;
use crate::types::{Signature, Ty};

use super::diagnostics::IllegalCtxtAccessKind;
use super::*;

impl BodyValidator<'_> {
    pub(super) fn validate(mut self) -> Vec<BodyDiagnostic> {
        for stmt in &self.body.entry_stmts {
            self.validate_stmt(*stmt)
        }
        for (branch, exprs) in self.trivial_probes {
            for (stmt, expr) in exprs {
                let diag = BodyDiagnostic::TrivialBranchAccess { branch, expr, stmt };
                self.diagnostics.push(diag);
            }
        }
        self.diagnostics
    }

    fn validate_stmt(&mut self, stmt: StmtId) {
        match self.body.stmts[stmt] {
            Stmt::Missing | Stmt::Empty => (),
            Stmt::Expr(expr) => self.validate_expr(expr, stmt),
            Stmt::Block { ref body } => body.iter().for_each(|stmt| self.validate_stmt(*stmt)),
            Stmt::Assignment { dst, val, op_kind } => {
                self.validate_expr(val, stmt);
                if op_kind == ast::AssignOp::Contribute && !self.ctxt.contribute_allowed() {
                    self.diagnostics
                        .push(BodyDiagnostic::IllegalContribute { stmt, ctxt: self.ctxt })
                } else if self.infer.assignment_destination.contains_key(&stmt) {
                    // avoid duplicate errors
                    self.validate_assignment_dst(dst, stmt);
                }
            }
            Stmt::If { cond, .. }
            | Stmt::ForLoop { cond, .. }
            | Stmt::WhileLoop { cond, .. }
            | Stmt::Case { discr: cond, .. } => {
                self.validate_condition(cond, stmt, |validator| {
                    validator.body.stmts[stmt]
                        .walk_child_stmts(|stmt| validator.validate_stmt(stmt))
                });
            }
            Stmt::EventControl { body, .. } => {
                let old = mem::replace(&mut self.ctxt, BodyContext::EventControl);
                self.validate_stmt(body);
                self.ctxt = old;
            }
        }
    }

    fn validate_expr(&mut self, expr: ExprId, stmt: StmtId) {
        ExprValidator { parent: self, is_write: false, stmt, sink: None }.validate_expr(expr)
    }

    fn validate_assignment_dst(&mut self, expr: ExprId, stmt: StmtId) {
        ExprValidator { parent: self, is_write: true, stmt, sink: None }.validate_expr(expr)
    }

    /// Return a list of non-constant dominators (conditions).
    fn validate_condition(
        &mut self,
        cond: ExprId,
        stmt: StmtId,
        f: impl FnOnce(&mut Self),
    ) -> Option<Box<[ExprId]>> {
        if matches!(self.ctxt, BodyContext::AnalogBlock | BodyContext::Conditional) {
            let mut non_const_access = Vec::new();
            ExprValidator {
                parent: self,
                stmt,
                is_write: false,
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

impl ExprValidator<'_, '_> {
    fn validate_expr(&mut self, expr: ExprId) {
        match self.parent.body.exprs[expr] {
            Expr::Path { port: false, .. } => {
                match self.parent.infer.expr_types[expr] {
                    Ty::Param(_, param) => {
                        if let ItemWithBodyId::ParamId(def) = self.parent.item {
                            if def.lookup(self.parent.db.upcast()).id
                                < param.lookup(self.parent.db.upcast()).id
                            {
                                self.report(BodyDiagnostic::IllegalParamAccess { def, expr, param })
                            }
                        }
                    }
                    Ty::Var(_, var) => {
                        self.check_context_access(expr, self.parent.ctxt.var_ref_allowed(), |_| {
                            IllegalCtxtAccessKind::Var(var)
                        });
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
                    _ => (),
                };
                return;
            }
            Expr::Call { ref fun, ref args, .. } => {
                match self.parent.infer.resolved_calls.get(&expr) {
                    Some(ResolvedFun::BuiltIn(builtin)) => {
                        let name = fun.as_ref().and_then(|p| p.as_ident()).unwrap();
                        let signature = self.parent.infer.resolved_signatures.get(&expr);
                        self.validate_builtin(*builtin, args, name, expr, signature.cloned());
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
                    self.parent.validate_condition(cond, self.stmt, |body_validator| {
                        let mut validator = ExprValidator {
                            parent: body_validator,
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
                if let Some(discipline) = Self::node_pair_discipline(self.parent.db, node1, node2) {
                    let node1 = self.parent.db.node_data(node1);
                    let node2 = self.parent.db.node_data(node2);
                    self.report_illegal_nature_access(
                        format!("({}, {})", node1.name, node2.name),
                        discipline,
                        Some(access_nature),
                        access_expr,
                    )
                } else {
                    self.report(BodyDiagnostic::IncompatibleUnnamedBranch {
                        expr: access_expr,
                        node1,
                        node2,
                    })
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

    fn report_illegal_nature_access(
        &mut self,
        branch: String,
        discipline: DisciplineId,
        access_nature: Option<NatureId>,
        access_expr: ExprId,
    ) {
        let db = self.parent.db;
        let discipline = db.discipline_info(discipline);

        let get_nature_info = |nature: NatureId| {
            let nature = nature.lookup(db);
            let nature = &nature.item_tree(db)[nature.id];
            Some((nature.name.clone(), nature.access.clone()?.0))
        };
        let pot = discipline.potential.and_then(get_nature_info);
        let flow = discipline.flow.and_then(get_nature_info);

        self.parent.diagnostics.push(BodyDiagnostic::IncompatibleNatureAccess {
            candidates: [pot, flow],
            access_nature,
            access_expr,
            branch,
        })
    }

    fn validate_builtin(
        &mut self,
        builtin: BuiltIn,
        args: &[ExprId],
        name: Name,
        expr: ExprId,
        signature: Option<Signature>,
    ) {
        match builtin {
            _ if builtin.is_unsupported() => self
                .parent
                .diagnostics
                .push(BodyDiagnostic::UnsupportedFunction { expr, func: builtin }),

            BuiltIn::potential | BuiltIn::flow => {
                self.check_context_access(expr, self.parent.ctxt.nature_access_allowed(), |_| {
                    IllegalCtxtAccessKind::NatureAccess
                })
            }

            _ if builtin.is_analog_operator() && builtin != BuiltIn::ddx
                || builtin.is_analog_operator_sysfun() =>
            {
                self.check_context_access(expr, self.parent.ctxt.analog_operator_allowed(), |sel| {
                    IllegalCtxtAccessKind::AnalogOperator {
                        name,
                        is_standard: builtin.is_analog_operator(),
                        non_const_dominator: sel.parent.non_const_dominator.clone(),
                    }
                })
            }

            _ if builtin.is_analysis_fun() => {
                self.check_context_access(expr, self.parent.ctxt.analysis_fun_allowed(), |_| {
                    IllegalCtxtAccessKind::AnalysisFun { name }
                })
            }

            _ => (),
        }

        let mut args = args;
        match (builtin, signature) {
            (BuiltIn::potential | BuiltIn::flow, Some(NATURE_ACCESS_NODES)) => {
                let hi = self.parent.infer.expr_types[args[0]].unwrap_node();
                let lo = self.parent.infer.expr_types[args[1]].unwrap_node(); // Fixed(JW): should be args[1]
                self.lint_trivial_branch(
                    if hi >= lo {
                        BranchWrite::Unnamed { hi, lo: Some(lo) }
                    } else {
                        BranchWrite::Unnamed { hi: lo, lo: Some(hi) }
                    },
                    builtin,
                    expr,
                );
                if let Some(discipline) = Self::node_pair_discipline(self.parent.db, hi, lo) {
                    self.validate_flow_or_pot(expr, builtin, discipline)
                } else {
                    self.report(BodyDiagnostic::IncompatibleUnnamedBranch {
                        expr,
                        node1: hi,
                        node2: lo,
                    });
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
                    self.validate_const_expr(*const_expr);
                    args = other_args; // Do not type check const expr twice
                };
            }

            // JW: these builtins are unsupported
            // (
            //     BuiltIn::laplace_nd
            //     | BuiltIn::laplace_np
            //     | BuiltIn::laplace_zp
            //     | BuiltIn::laplace_zd
            //     | BuiltIn::zi_nd
            //     | BuiltIn::zi_np
            //     | BuiltIn::zi_zd
            //     | BuiltIn::zi_zp,
            //     Some(_),
            // ) => {
            //     if let [_expr, const_args @ ..] = args {
            //         args = &args[..1];
            //         for arg in const_args {
            //             self.validate_const_expr(*arg)
            //         }
            //     }
            // }
            _ => (),
        }

        // Validate the arguments at last.
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

    /// Trivial branches are those probed but do not receive any contributions.
    fn lint_trivial_branch(&mut self, branch: BranchWrite, call: BuiltIn, expr: ExprId) {
        let is_flow = call == BuiltIn::flow;
        if self.is_write {
            self.parent.non_trivial_branches.insert(branch);
            self.parent.trivial_probes.remove(&branch);
        } else if is_flow && !self.parent.non_trivial_branches.contains(&branch) {
            self.parent.trivial_probes.entry(branch).or_default().push((self.stmt, expr))
        }
    }

    fn node_pair_discipline(
        db: &dyn HirTyDB,
        node_1: NodeId,
        node_2: NodeId,
    ) -> Option<DisciplineId> {
        let d_1 = db.node_discipline(node_1)?;
        let d_2 = db.node_discipline(node_2)?;
        db.discipline_info(d_2).compatible(d_1, db).then_some(d_1)
    }

    fn validate_flow_or_pot(&mut self, expr: ExprId, call: BuiltIn, discipline: DisciplineId) {
        let is_pot = call == BuiltIn::potential;
        let discipline = self.parent.db.discipline_info(discipline);
        if discipline.potential.is_none() && is_pot || discipline.flow.is_none() && !is_pot {
            self.report(BodyDiagnostic::IllegalNatureAccess { is_pot, expr })
        }
    }

    fn check_context_access(
        &mut self,
        expr: ExprId,
        allowed: bool,
        kind: impl FnOnce(&Self) -> IllegalCtxtAccessKind,
    ) {
        if let Some(sink) = &mut self.sink {
            sink.push(expr)
        }
        if !allowed {
            self.report(BodyDiagnostic::IllegalCtxtAccess {
                kind: kind(self),
                ctxt: self.parent.ctxt,
                expr,
            });
        }
    }

    #[inline(always)]
    fn report(&mut self, diag: BodyDiagnostic) {
        self.parent.diagnostics.push(diag)
    }
}
