//! Determine the evaluation type of an analog operator (ddt, noise), that is,
//! whether it requires to add an implicit unknown, or it can be linearized,
//! that is, the contribution can be mapped to a separate dimension.

use std::mem;

use bitset::SparseBitMatrix;
use hir_lower::{CallBackKind, HirInterner, ImplicitEquationKind, ParamKind, PlaceKind};
use mir::builder::InstBuilder;
use mir::cursor::{Cursor, FuncCursor};
use mir::{
    Block, ControlFlowGraph, FuncRef, Function, Inst, InstructionData, Opcode, PhiNode, Postorder,
    Value, FALSE, F_ONE, F_ZERO, TRUE,
};
use typed_indexmap::TiSet;

use super::{Contribution, Noise};
use crate::util::{add, update_optbarrier};

/// The evaluation type of an analog operator
#[derive(Debug)]
pub(super) enum Evaluation {
    /// The analog operator must be evaluated as a separate equation
    Equation,
    /// The analog operator can be evaluated as a linear contribution
    /// without the need for an additional unknown
    Linear {
        /// The contribute that this linear equation writes to.
        /// - the first is the original contribution
        /// - the second is the separate dimension it is mapped to
        contributes: Box<[(Value, Value)]>,
    },
    /// This operator is not used and can be ignored
    Dead,
}

impl super::Builder<'_> {
    /// Build topology for analog operators (ddt, noise) with predetermined evaluation types
    pub(super) fn build_analog_operators(
        &mut self,
        cfg: &ControlFlowGraph,
        pdf: &SparseBitMatrix<Block, Block>,
        intern: &mut HirInterner,
    ) {
        let mut ssa_builder = mir_build::SSAVariableBuilder::new(cfg);
        for (operator, evaluation) in self.analog_operator_evaluations(pdf, intern) {
            let arg0 = self.func.dfg.instr_args(operator)[0];
            let cb = self.func.dfg.func_ref(operator).unwrap();
            let is_noise = intern.callbacks[cb].is_noise();

            match evaluation {
                Evaluation::Dead => {
                    cov_mark::hit!(dead_noise);
                    let val = self.func.dfg.first_result(operator);
                    self.func.dfg.replace_uses(val, F_ZERO);
                }
                Evaluation::Linear { contributes } => {
                    cov_mark::hit!(linear_operator);
                    let cb = &intern.callbacks[cb];
                    for (val, dim) in contributes {
                        let inst = self.func.dfg.value_def(val).inst().unwrap();
                        let kind = self.as_contribute_kind(val).unwrap();
                        let contribution = self.topology.get_contrib_mut(kind);
                        if is_noise {
                            let dim = FuncCursor::new(self.func)
                                .after_inst(inst)
                                .ins()
                                .ensure_optbarrier(dim);
                            let noise = Noise::new(operator, cb, dim, &mut ssa_builder, self.func);
                            contribution.noises.push(noise)
                        } else {
                            update_optbarrier(
                                self.func,
                                &mut contribution.react,
                                |mut val, cursor| {
                                    add(cursor, &mut val, dim, false);
                                    val
                                },
                            );
                        }
                    }
                }
                Evaluation::Equation => {
                    let eq_kind = if is_noise {
                        ImplicitEquationKind::NoiseSrc
                    } else {
                        ImplicitEquationKind::Ddt
                    };
                    let eq = intern.implicit_equations.push_and_get_key(eq_kind);
                    // new implicit unknown (that is passed as parameter into evaluation
                    // function) is added to replace the result of analog operator
                    let eq_val =
                        intern.ensure_param(&mut self.func, ParamKind::ImplicitUnknown(eq));
                    let res = self.func.dfg.first_result(operator);
                    self.func.dfg.replace_uses(res, eq_val);

                    let collapse = ssa_builder.define_at_exit(self.func, TRUE, FALSE, operator);
                    if collapse != FALSE {
                        cov_mark::hit!(collapsible_implicit);
                        debug_assert_ne!(collapse, TRUE);
                        intern
                            .outputs
                            .insert(PlaceKind::CollapseImplicitEquation(eq), collapse.into());
                    }

                    // TODO(JW): Why negate the resistive residual here?
                    let neg_eq_val = FuncCursor::new(self.func).at_exit().ins().fneg(eq_val);
                    let contrib = if is_noise {
                        self.topology.small_signal_vals.insert(eq_val);
                        let noise = Noise::new(
                            operator,
                            &intern.callbacks[cb],
                            F_ONE,
                            &mut ssa_builder,
                            self.func,
                        );
                        Contribution {
                            unknown: Some(eq_val),
                            resist: neg_eq_val,
                            noises: vec![noise],
                            ..Contribution::default()
                        }
                    } else {
                        let arg0 = ssa_builder.define_at_exit(self.func, F_ZERO, arg0, operator);
                        Contribution {
                            unknown: Some(eq_val),
                            resist: neg_eq_val,
                            react: arg0,
                            ..Contribution::default()
                        }
                    };

                    self.new_implicit_equation(eq, contrib);
                }
            }
            // not needed anymore, wipe the callback
            self.func.dfg.zap_inst(operator);
            self.func.layout.remove_inst(operator);
        }
    }

    /// Determine the evaluation type of all the analog operators.
    fn analog_operator_evaluations(
        &mut self,
        postdom_frontiers: &SparseBitMatrix<Block, Block>,
        intern: &mut HirInterner,
    ) -> Vec<(Inst, Evaluation)> {
        let mut analog_operators = Vec::new();

        // First, iterate all analog operators and determine if they can be
        // lineraized/turned into dimensions. This step does not modify the
        // function yet, as otherwise the detection may return incorrect results
        for (cb, insts) in intern.callback_callers.iter_mut_enumerated() {
            match intern.callbacks[cb] {
                CallBackKind::TimeDerivative => {
                    for inst in mem::take(insts) {
                        if self.func.layout.inst_block(inst).is_none() {
                            // ddt() has been dead code eliminated
                            continue;
                        }
                        if self.func.dfg.is_safe_to_remove(inst)
                            || !self.op_dependent_insts.contains(inst)
                        // ddt() returns zero when its argument is not op-dependent
                        {
                            let result = self.func.dfg.first_result(inst);
                            self.func.dfg.replace_uses(result, F_ZERO);
                            self.func.dfg.zap_inst(inst);
                            self.func.layout.remove_inst(inst);
                            continue;
                        }
                        let eval_ty = self.determine_evaluation_type(
                            inst,
                            postdom_frontiers,
                            &intern.callbacks,
                            false,
                        );
                        analog_operators.push((inst, eval_ty));
                    }
                }
                CallBackKind::WhiteNoise { .. }
                | CallBackKind::FlickerNoise { .. }
                | CallBackKind::NoiseTable(_) => {
                    for inst in mem::take(insts) {
                        let eval_ty = self.determine_evaluation_type(
                            inst,
                            postdom_frontiers,
                            &intern.callbacks,
                            true,
                        );
                        analog_operators.push((inst, eval_ty));
                    }
                }
                _ => (),
            }
        }
        analog_operators
    }

    /// Basic nodal KCL form: `f(x(t)) + ddt(q(x(t))) = u(t)`
    /// does not support terms of the form `g(x(t)) * ddt(h(x(t)))`
    ///
    /// Therefore, conrtibution statements like these:
    ///
    /// ```text
    /// x = V(a, b);
    /// I(a, b) <+ g(x) * ddt(h(x));
    /// ```
    /// do not align with basic KCL nodal formulation, and requires an
    /// extra state variable`phi`, which satisfy implicit equation:
    ///
    /// ```text
    /// phi - ddt(h(x)) = 0
    /// ```
    /// which fits into the `f(x(t)) + ddt(q(x(t)))` form.
    ///
    /// The compiler should introduce an implicit equation when:
    /// - `ddt()`'s are multiplied/divided with op-dependent values
    /// - variables that depend on `ddt()` are used in op-dependent conditionals
    fn determine_evaluation_type(
        &mut self,
        analog_operator_call: Inst, // the call instruction of analog operator
        postdom_frontiers: &SparseBitMatrix<Block, Block>,
        callbacks: &TiSet<FuncRef, CallBackKind>,
        noise: bool,
    ) -> Evaluation {
        let Self { func, output_values, .. } = self;

        let mut traversal = Postorder::new(&func.dfg, |_| true).at_inst(analog_operator_call);
        let postorder = traversal.by_ref().collect::<Box<_>>();
        let visited = traversal.visited();
        let val_visited =
            &|val| func.dfg.value_def(val).inst().is_some_and(|inst| visited.contains(inst));
        let is_op_dependent = |val| {
            if let Some(inst) = func.dfg.value_def(val).inst() {
                self.op_dependent_insts.contains(inst)
            } else {
                self.op_dependent_vals.contains(&val)
            }
        };
        let mut contributes = Vec::new();

        use {InstructionData::*, Opcode::*};
        for &inst in &postorder {
            match func.dfg.insts[inst] {
                Unary { opcode: Fneg, .. } | Binary { opcode: Fadd | Fsub, .. } => {}

                Binary { opcode: Fmul, args } => {
                    if is_op_dependent(args[0]) && is_op_dependent(args[1]) {
                        return Evaluation::Equation;
                    }
                }
                Binary { opcode: Fdiv, args } => {
                    if is_op_dependent(args[1]) {
                        // TODO(JW): why arg0 is not considered here?
                        return Evaluation::Equation;
                    }
                }

                // For noise, phis don't matter at all.
                // since it's a small signal value (so doesn't need to be consistently
                // maintained across multiple iterations). I am not quite sure if this
                // plays nice with transient noise and other more advanced simulation
                // types but I can't see why it wouldn't (also the language standard
                // specifically calls these small signal sources).
                PhiNode(_) | Branch { .. } | Binary { opcode: Flt | Fle | Fgt | Fge, .. }
                    if noise => {}

                // Noise is always zero when these are evaluated.
                // TODO: complex noise power (would allow us to avoid creating an extra node here)
                InstructionData::Call { func_ref, .. }
                    if noise && callbacks[func_ref] != CallBackKind::TimeDerivative => {}

                PhiNode(ref phi) => {
                    // Check if a phi is operating point dependent. To determine that
                    // we check whether any of the control dependencies of phi edge
                    // is operating point dependent.
                    let phi_op_dependent = |phi| {
                        for (pred, _) in func.dfg.phi_edges(phi) {
                            let Some(control_deps) = postdom_frontiers.row(pred) else { continue };
                            for control_dep in control_deps.iter() {
                                if func
                                    .layout
                                    .block_terminator(control_dep)
                                    .and_then(|inst| func.dfg.branch_cond(inst))
                                    .is_some_and(is_op_dependent)
                                {
                                    return true;
                                }
                            }
                        }
                        false
                    };

                    // However, to avoid generating too many unnecessary implicit equations,
                    // a special optimization for chains of additions is necessary.
                    // Chains of addition/subtraction where only one summand depends on the
                    // analog operator, do not require an extra equation.
                    //
                    // This optimizes the following (common) case:
                    //
                    // I(branch) <+ ddt(V(a));
                    // if (op_dependent_cond)
                    //    I(branch) <+ foo;
                    //
                    // This will create an op-dependent phi [ddt(V(a)), ddt(V(a))+foo].
                    // This does not change the ddt state and therefore doesn't require an
                    // extra implicit equation.
                    self.val_map.clear();
                    let handle_loops = &mut |phi, enter| {
                        if enter {
                            self.val_map.insert(phi, F_ZERO).is_none()
                        } else {
                            self.val_map.remove(&phi).is_some()
                        }
                    };
                    if phi_op_dependent(phi)
                        && phi_add_chain_start(func, phi, val_visited, handle_loops).is_none()
                    {
                        cov_mark::hit!(conditional_phi);
                        return Evaluation::Equation;
                    }
                }

                Unary { opcode: OptBarrier, .. } if noise => {
                    let val = func.dfg.first_result(inst);
                    if let Some(contrib_kind) = self.contrib_map.get(&val) {
                        if !contrib_kind.is_reactive() && contributes.is_empty() {
                            contributes.push(val);
                        } else {
                            // multiple uses of a noise source means correlated noise,
                            // for now just create a correlation network
                            return Evaluation::Equation;
                        }
                    }
                }
                Unary { opcode: OptBarrier, .. } => {
                    let val = func.dfg.first_result(inst);
                    if output_values.contains(val) {
                        if self.contrib_map.get(&val).is_some_and(|it| !it.is_reactive()) {
                            contributes.push(val);
                        } else {
                            return Evaluation::Equation;
                        }
                    }
                }
                _ => return Evaluation::Equation,
            }
        }
        if contributes.is_empty() {
            debug_assert!(noise, "ddt should have been deadcode eliminated");
            return Evaluation::Dead; // unused noise
        }
        // Now that the analog operator can be linearized, we can create another dimension
        // of the contribution, a mapping from the original instruction result value to
        // another SSA value.
        let res = self.func.dfg.first_result(analog_operator_call);
        let arg = if noise { F_ONE } else { self.func.dfg.instr_args(analog_operator_call)[0] };
        self.create_dimension(res, arg, &postorder);

        let contributes = contributes.into_iter().map(|val| (val, self.val_map[&val])).collect();

        Evaluation::Linear { contributes }
    }
}

fn phi_add_chain_start(
    func: &Function,
    phi: &PhiNode,
    val_visited: &impl Fn(Value) -> bool,
    handle_loops: &mut impl FnMut(Value, bool) -> bool,
) -> Option<Value> {
    let mut chain_start = None;
    for (_, mut edge) in func.dfg.phi_edges(phi) {
        chain_start = val_visited(edge).then(|| {
            edge = follow_add_chain(func, edge, val_visited, handle_loops);
            match chain_start {
                Some(start) if start != edge => None,
                None => Some(edge),
                _ => chain_start,
            }
        })?;
    }
    chain_start
}

fn follow_add_chain(
    func: &Function,
    mut val: Value,
    val_visited: &impl Fn(Value) -> bool,
    handle_loops: &mut impl FnMut(Value, bool) -> bool,
) -> Value {
    while let Some(inst) = func.dfg.value_def(val).inst() {
        match func.dfg.insts[inst] {
            InstructionData::Binary { opcode: Opcode::Fadd, args: [lhs, rhs] } => {
                if !val_visited(lhs) {
                    val = rhs;
                    continue;
                }
                if !val_visited(rhs) {
                    val = lhs;
                    continue;
                }
            }
            InstructionData::Binary { opcode: Opcode::Fsub, args: [lhs, rhs] } => {
                if !val_visited(rhs) {
                    val = lhs;
                    continue;
                }
            }
            InstructionData::PhiNode(ref phi) => {
                if handle_loops(val, true) {
                    let start = phi_add_chain_start(func, phi, val_visited, handle_loops);
                    handle_loops(val, false);
                    if let Some(start) = start {
                        val = start;
                        continue;
                    }
                }
            }
            _ => (),
        }
        break;
    }
    val
}
