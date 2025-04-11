// TODO:(JW): according to LRM, The value returned by any branch flow probe in the analog block,
// shall be divided by $mfactor.
//
// This means when `I(br)` appears at RHS of some expression, its value should be divided by
// $mfactor, but it seems that OpenVAF do not handle this so far.
//
// However, for compact models, `I<br>` seldomly appears at RHS.

use std::mem;

use ahash::AHashMap;
use bitset::BitSet;
use hir::{BranchWrite, CompilationDB, Node, ParamSysFun};
use hir_lower::{HirInterner, ImplicitEquation, ParamKind};
use indexmap::IndexSet;
use mir::builder::InstBuilder;
use mir::cursor::{Cursor, FuncCursor};
use mir::{Block, Inst, Unknown, Value, F_ONE, F_ZERO};
use mir::{ControlFlowGraph, DominatorTree, KnownDerivatives};
use mir_autodiff::auto_diff;
use typed_index_collections::TiVec;

use crate::context::Context;
use crate::noise::NoiseSource;
use crate::topology::{BranchInfo, Contribution, Noise};
use crate::util::{self, add, is_op_dependent, strip_optbarrier, update_optbarrier};
use crate::WITHOUT_CONTRIBUTES;

use super::{DaeSystem, MatrixEntry, Residual, SimUnknown, SimUnknownKind};

const POS: bool = false;
const NEG: bool = true;

impl Residual {
    /// Add/subtract `contrib` to/from this residual
    fn add_contrib<const NEGATE: bool>(&mut self, contrib: &Contribution, cursor: &mut FuncCursor) {
        let mut add = |residual: &mut Value, contrib: Value| {
            let contrib = strip_optbarrier(&mut *cursor, contrib);
            util::add(cursor, residual, contrib, NEGATE)
        };
        add(&mut self.resist, contrib.resist);
        add(&mut self.react, contrib.react);
        add(&mut self.resist_small_signal, contrib.resist_small_signal);
        add(&mut self.react_small_signal, contrib.react_small_signal);
    }

    /// Add/subtract `val` to/from the resistive part of this residual
    ///
    /// This should be used when dealing with additional source equation/sim_unknown
    fn add_unknown<const NEGATE: bool>(&mut self, mut val: Value, cursor: &mut FuncCursor) {
        val = strip_optbarrier(&cursor, val);
        util::add(cursor, &mut self.resist, val, NEGATE);
    }
}

pub(super) struct Builder<'a> {
    pub(super) dae: DaeSystem,
    pub(super) db: &'a CompilationDB,
    pub(super) cursor: FuncCursor<'a>,
    pub(super) intern: &'a mut HirInterner,
    pub(super) cfg: &'a mut ControlFlowGraph,
    pub(super) dom_tree: &'a mut DominatorTree,
    pub(super) op_dependent_insts: &'a BitSet<Inst>,
    pub(super) output_values: &'a mut BitSet<Value>,
}

impl<'a> Builder<'a> {
    pub(super) fn new(ctx: &'a mut Context) -> Self {
        ctx.compute_outputs::<WITHOUT_CONTRIBUTES>();
        let mut builder = Self {
            dae: DaeSystem::default(),
            db: ctx.db,
            cursor: FuncCursor::new(&mut ctx.func).at_exit(),
            intern: &mut ctx.intern,
            cfg: &mut ctx.cfg,
            dom_tree: &mut ctx.dom_tree,
            op_dependent_insts: &ctx.op_dependent_insts,
            output_values: &mut ctx.output_values,
        };
        // ensure ports are the first unknowns and always have an unknown
        for port in ctx.module.module.ports(builder.db) {
            builder.build_node(port)
        }
        for node in ctx.module.module.internal_nodes(builder.db) {
            builder.build_node(node)
        }

        builder
    }

    pub(super) fn with_small_signal_network(
        mut self,
        small_signal_vals: IndexSet<Value, ahash::RandomState>,
    ) -> Self {
        self.dae.small_signal_params = small_signal_vals;
        self
    }

    /// Consume the builder and generate the DAE system
    pub(super) fn finish(mut self) -> DaeSystem {
        let sim_unknown_reads = self.sim_unknown_reads();
        let derivative_info = self.intern.derivative_info(&self.cursor, true);
        let jacobian_info =
            self.jacobian_info(sim_unknown_reads.iter().map(|&(_, val)| val), &derivative_info);

        // TODO(perf): incrementally update dom_tree (for switch branches) instead
        self.dom_tree.compute::<true, false>(self.cursor.func, self.cfg);

        // Actually generate the instructions and values for derivatives
        let derivatives =
            auto_diff(&mut *self.cursor.func, self.dom_tree, &derivative_info, &jacobian_info);
        drop(jacobian_info);

        // auto_diff may in an unlikely case add extra bb at the end, ensure we are building everything at the end
        self.cursor.goto_exit();

        // Now that we have all the derivatives, we can build the DAE.
        self.build_jacobian(&sim_unknown_reads, &derivative_info, &derivatives);
        self.build_lim_rhs(&derivative_info, derivatives);

        // Ensure the optbarriers for DAE related SSA values and apply $mfactor
        self.ensure_optbarriers();

        self.dae
    }
}

/// Get the residual entry corresponding to given simulation unknown.
///
/// If the residual entry does not exist, allocate a new one and return it.
macro_rules! get_residual {
    ($self: ident, $unknown: expr) => {{
        let unknown = $self.ensure_unknown($unknown);
        &mut $self.dae.residual[unknown]
    }};
}

impl Builder<'_> {
    fn ensure_unknown(&mut self, unknown: SimUnknownKind) -> SimUnknown {
        let (unknown, new) = self.dae.unknowns.ensure(unknown);
        if new {
            self.dae.residual.push(Residual::default());
        }
        unknown
    }

    pub(super) fn build_node(&mut self, node: Node) {
        self.ensure_unknown(SimUnknownKind::KirchhoffLaw(node));
    }

    pub(super) fn build_branch(&mut self, dst: BranchWrite, branch: &BranchInfo) {
        match branch.is_potential {
            // flow source branch
            // only flow(branch) is assigned (appears at contribution statement LHS)
            mir::FALSE => {
                // Get the flow contribution
                let contrib = self.flow_contribution(branch);

                // If the flow of branch is probed (appears at contribution statement RHS),
                // we should make the flow an extra simulation unknown
                let flow_probed =
                    self.intern.is_param_live(&self.cursor, &ParamKind::Flow(dst.into()));

                if flow_probed {
                    self.add_source_equation(&contrib, branch.flow.unknown.unwrap(), dst);
                } else {
                    self.add_kirchhoff_law(&contrib, dst);
                }
            }
            // potential source branch
            // only potential(branch) is assigned (appears at contribution statement LHS)
            mir::TRUE => {
                // A potential source cannot be described with KFL. According to MNA,
                // the flow through the potential source's branch should be included
                // as an extra simulation unknown.

                // If the flow of branch is probed (appears at contribution statement RHS),
                // we should make the flow an extra simulation unknown
                let flow_probed =
                    self.intern.is_param_live(&self.cursor, &ParamKind::Flow(dst.into()));

                // Branches only used for node collapse look like pure potential sources,
                // but has zero potential contributions, e.g. V(a, b) < 0.0
                // Make sure to ignore these branches.
                let is_collapse_hint = branch.potential.is_trivial();

                if flow_probed || !is_collapse_hint {
                    let contrib = self.potential_contribution(branch);
                    self.add_source_equation(&contrib, branch.flow.unknown.unwrap(), dst);
                }
            }
            // switch source branch
            // both flow(branch) and potential(branch) are assigned (appear at contribution statement LHS)
            _ => {
                // In most cases, what looks like a switch branch is just node collapse hint.
                // Make sure we don't create switch branches when they aren't needed.

                // Is the source branch type op dependent?
                let is_op_dependent = is_op_dependent(
                    branch.is_potential,
                    &self.cursor,
                    self.intern,
                    self.op_dependent_insts,
                );
                // Is the flow of branch probed?
                let flow_probed =
                    self.intern.is_param_live(&self.cursor, &ParamKind::Flow(dst.into()));
                // Is the potential contribution zero?
                let is_collapse_hint = branch.potential.is_trivial();

                if is_op_dependent || flow_probed || !is_collapse_hint {
                    // An actual switch branch
                    let start_bb = self.cursor.current_block().unwrap();
                    let potential_bb = self.cursor.layout_mut().append_new_block();
                    let next_block = self.cursor.layout_mut().append_new_block();
                    self.cfg.ensure_block(next_block);
                    self.cfg.add_edge(start_bb, potential_bb);
                    self.cfg.add_edge(start_bb, next_block);
                    self.cfg.add_edge(potential_bb, next_block);

                    // Debugging
                    // println!("start bb {:?}", start_bb);
                    // println!("voltage src bb {:?}", potential_bb);
                    // println!("next block {:?}", next_block);
                    // println!("cursor at {:?}", self.cursor.position());

                    // Get the condition that determines if branch acts as a voltage source
                    // Skip trailing optbarriers
                    let is_potential = strip_optbarrier(&self.cursor, branch.is_potential);
                    // If condition is true, jump to potential_bb, or else go to next_block
                    self.cursor.ins().br(is_potential, potential_bb, next_block);
                    self.cursor.goto_bottom(potential_bb);
                    self.cursor.ins().jump(next_block);
                    self.cursor.goto_bottom(next_block);
                    let contrib = self.switch_branch_contribution(branch, potential_bb, start_bb);
                    self.add_source_equation(&contrib, branch.flow.unknown.unwrap(), dst)
                } else {
                    // Not a real switch branch
                    let contrib = self.flow_contribution(branch);
                    self.add_kirchhoff_law(&contrib, dst);
                }
            }
        };
    }

    pub(super) fn build_implicit_equation(&mut self, eq: ImplicitEquation, contrib: &Contribution) {
        get_residual!(self, SimUnknownKind::Implicit(eq))
            .add_contrib::<POS>(contrib, &mut self.cursor);
    }

    fn flow_contribution(&mut self, BranchInfo { flow, .. }: &BranchInfo) -> Contribution {
        let mfactor = self
            .intern
            .ensure_param(&mut self.cursor, ParamKind::ParamSysFun(ParamSysFun::mfactor));
        let noises = flow
            .noises
            .iter()
            .map(|src| {
                let mut src = src.clone();
                src.factor = self.multiply_sqrt_mfactor(mfactor, src.factor);
                src
            })
            .collect();

        Contribution {
            unknown: flow.unknown,
            resist: flow.resist,
            react: flow.react,
            resist_small_signal: flow.resist_small_signal,
            react_small_signal: flow.react_small_signal,
            noises,
        }
    }

    fn potential_contribution(
        &mut self,
        BranchInfo { potential, .. }: &BranchInfo,
    ) -> Contribution {
        let mfactor = self
            .intern
            .ensure_param(&mut self.cursor, ParamKind::ParamSysFun(ParamSysFun::mfactor));
        let noises = potential
            .noises
            .iter()
            .map(|src| {
                let mut src = src.clone();
                src.factor = self.divide_sqrt_mfactor(mfactor, src.factor);
                src
            })
            .collect();

        Contribution {
            unknown: potential.unknown,
            resist: potential.resist,
            react: potential.react,
            resist_small_signal: potential.resist_small_signal,
            react_small_signal: potential.react_small_signal,
            noises,
        }
    }

    fn switch_branch_contribution(
        &mut self,
        BranchInfo { potential, flow, .. }: &BranchInfo,
        potential_bb: Block,
        flow_bb: Block,
    ) -> Contribution {
        let mut select = |potential_val, flow_val| {
            let potential_val = strip_optbarrier(&self.cursor, potential_val);
            let flow_val = strip_optbarrier(&self.cursor, flow_val);
            if potential_val == flow_val {
                potential_val
            } else {
                self.cursor.ins().phi1(&[(flow_bb, flow_val), (potential_bb, potential_val)])
            }
        };

        let unknown = select(potential.unknown.unwrap(), flow.unknown.unwrap());

        // Build noise phi nodes
        let mut noises = Vec::with_capacity(potential.noises.len() + flow.noises.len());

        // For each noise add a phi instruction that joins the values for the case the
        // switch branch behaves as a voltage source (source value) and as a current source (0)
        let potential_noise = potential.noises.iter().map(|src| {
            let mut src = src.clone();
            src.factor = select(src.factor, F_ZERO);
            src
        });
        noises.extend(potential_noise);

        // For each noise add a phi instruction that joins the values for the case the
        // switch branch behaves as a voltage source (0) and as a current source (source value)
        let flow_noise = flow.noises.iter().map(|src| {
            let mut src = src.clone();
            src.factor = select(F_ZERO, src.factor);
            src
        });
        noises.extend(flow_noise);

        // Build remaining phi nodes
        let phi_resist = select(potential.resist, flow.resist);
        let phi_react = select(potential.react, flow.react);
        let phi_resist_ss = select(potential.resist_small_signal, flow.resist_small_signal);
        let phi_react_ss = select(potential.react_small_signal, flow.react_small_signal);

        // Scale noise signal with mfactor. Refer to [LRM 6.3.6] for rules of applying mfactor
        // This *must* be done after building all phis, since phis must be listed at block beginning
        let mfactor = self
            .intern
            .ensure_param(&mut self.cursor, ParamKind::ParamSysFun(ParamSysFun::mfactor));
        for ii in 0..potential.noises.len() + flow.noises.len() {
            if ii < potential.noises.len() {
                // Noise contributed to branch potential quantity
                noises[ii].factor = self.divide_sqrt_mfactor(mfactor, noises[ii].factor);
            } else {
                // Noise contributed to branch flow quantity
                noises[ii].factor = self.multiply_sqrt_mfactor(mfactor, noises[ii].factor);
            }
        }

        Contribution {
            unknown: Some(unknown),
            resist: phi_resist,
            react: phi_react,
            resist_small_signal: phi_resist_ss,
            react_small_signal: phi_react_ss,
            noises,
        }
    }

    /// Multiply the noise `srcfactor` with sqrt(mfactor).
    /// `srcfactor` is the signal scaling factor (not power scale factor).
    /// Since power scales with mfactor, the signal scales with sqrt(mfactor).
    fn multiply_sqrt_mfactor(&mut self, mfactor: Value, srcfactor: Value) -> Value {
        match (mfactor, srcfactor) {
            // Leave srcfactor unchanged if mfactor is 1
            (F_ONE, fac) => fac,
            (mfactor, srcfactor) => {
                let sqrt_mfactor = self.cursor.ins().sqrt(mfactor);
                if srcfactor == F_ONE {
                    // Old factor is 1, replace it with sqrt(mfactor)
                    sqrt_mfactor
                } else {
                    // Multiply old factor with sqrt(mfactor)
                    self.cursor.ins().fmul(srcfactor, sqrt_mfactor)
                }
            }
        }
    }

    /// Divide the noise `srcfactor` with sqrt(mfactor).
    /// `srcfactor` is the signal scaling factor (not power scale factor).
    /// Since power scales with mfactor, the signal scales with sqrt(mfactor).
    fn divide_sqrt_mfactor(&mut self, mfactor: Value, srcfactor: Value) -> Value {
        match (mfactor, srcfactor) {
            // Leave srcfactor unchanged if mfactor is 1
            (F_ONE, fac) => fac,
            (mfactor, srcfactor) => {
                let sqrt_mfactor = self.cursor.ins().sqrt(mfactor);
                self.cursor.ins().fdiv(srcfactor, sqrt_mfactor)
            }
        }
    }

    /// Add contributions according to Kirchhoff Flow Law (KFL), or KCL in terms of electrical discipline.
    fn add_kirchhoff_law(&mut self, contrib: &Contribution, dst: BranchWrite) {
        let (hi, lo) = dst.node_pair(self.db);
        let hi = SimUnknownKind::KirchhoffLaw(hi);
        let lo = lo.map(SimUnknownKind::KirchhoffLaw);
        get_residual!(self, hi).add_contrib::<POS>(contrib, &mut self.cursor);
        if let Some(lo) = lo {
            get_residual!(self, lo).add_contrib::<NEG>(contrib, &mut self.cursor);
        }
        self.add_noise(&contrib.noises, hi, lo);
    }

    /// Add contributions with respect to an extra equation according to Modified Nodal Analysis (MNA).
    /// The extra equation's unknown type is not nodal potential but branch flow.
    ///
    /// After allocating the extra equation, the formulation will become:
    ///
    /// ```text
    ///         V(p)   V(n)  ...  I(p,n)  |    RHS
    /// V(p)    J_pp   J_pn  ...   xxx    |  +I(p,n)
    /// V(n)    J_np   J_nn  ...   xxx    |  -I(p,n)
    /// ...     ...    ...   ...   ...    |    ...
    /// I(p,n)  xxx    xxx   ...    0     |  -V(p,n)
    /// ```
    fn add_source_equation(
        &mut self,
        contrib: &Contribution,
        branch_flow: Value,
        dst: BranchWrite,
    ) {
        let residual = get_residual!(self, SimUnknownKind::BranchFlow(dst.into()));
        // For additional equation residual: contribution -
        residual.add_contrib::<POS>(contrib, &mut self.cursor);
        residual.add_unknown::<NEG>(contrib.unknown.unwrap(), &mut self.cursor);
        self.add_noise(&contrib.noises, SimUnknownKind::BranchFlow(dst.into()), None);

        let (hi, lo) = dst.node_pair(self.db);
        let hi = SimUnknownKind::KirchhoffLaw(hi);
        let lo = lo.map(SimUnknownKind::KirchhoffLaw);
        get_residual!(self, hi).add_unknown::<POS>(branch_flow, &mut self.cursor);
        if let Some(lo) = lo {
            get_residual!(self, lo).add_unknown::<NEG>(branch_flow, &mut self.cursor);
        }
    }

    fn add_noise(&mut self, noises: &[Noise], hi: SimUnknownKind, lo: Option<SimUnknownKind>) {
        let hi = self.ensure_unknown(hi);
        let lo = lo.map(|lo| self.ensure_unknown(lo));
        self.dae.noise_sources.extend(noises.iter().map(|src| {
            let factor = src.factor;
            NoiseSource { name: src.name, kind: src.kind.clone(), hi, lo, factor }
        }))
    }

    /// Function parameters that read simulation unknowns, which need to be
    /// considered during Jacobi matrix construction.
    ///
    /// Construct from function parameters instead of simulation unknowns because
    /// voltage probes access two node voltages at the same time:
    ///
    /// `V(x, y) = V(x) - V(y)`
    ///
    /// We derive by *node voltage differences* to reduce the number of generated derivatives.
    fn sim_unknown_reads(&self) -> Vec<(ParamKind, Value)> {
        self.intern
            .live_params(&self.cursor.func.dfg)
            .filter_map(|(_, &kind, param)| match kind {
                ParamKind::Potential { .. }
                | ParamKind::Flow(_)
                | ParamKind::ImplicitUnknown(_) => Some((kind, param)),
                _ => None,
            })
            .collect()
    }

    /// Get the information of Jacobian derivatives implicitly required by the DAE system.
    ///
    /// Return the operand pair of ddx():
    /// - the dependent variable (`Residual`)
    /// - the independent variable (`mir::Unknown`)
    pub fn jacobian_info(
        &self,
        sim_unknowns: impl Iterator<Item = Value>,
        derivative_info: &KnownDerivatives,
    ) -> Vec<(Value, Unknown)> {
        let unknowns = sim_unknowns.filter_map(|param| derivative_info.unknowns.index_of(&param));
        let lim_unknowns = self.intern.lim_state.raw.values().flat_map(|vals| {
            vals.iter().filter_map(|(val, _)| {
                if !self.cursor.func.dfg.value_dead(*val) {
                    derivative_info.unknowns.index_of(val)
                } else {
                    None
                }
            })
        });
        let unknowns: Vec<_> = unknowns.chain(lim_unknowns).collect();

        let smsig_unknowns: Vec<_> = self
            .dae
            .small_signal_params
            .iter()
            .filter_map(|&param| derivative_info.unknowns.index_of(&param))
            .collect();

        // For each `(Residual, Unknown)` pair, we need 2 entries for both resistive and reactive Jacobi
        let mut jacobians = Vec::with_capacity(self.dae.residual.len() * unknowns.len() * 2);
        for residual in &self.dae.residual {
            // If residual is non-constant, then a Jacobian derivative is required
            if self.cursor.func.dfg.value_def(residual.resist).as_const().is_none() {
                jacobians.extend(unknowns.iter().map(|unk| (residual.resist, *unk)))
            }
            if self.cursor.func.dfg.value_def(residual.react).as_const().is_none() {
                jacobians.extend(unknowns.iter().map(|unk| (residual.react, *unk)))
            }
            // Handle small signal (e.g. noise)
            if self.cursor.func.dfg.value_def(residual.resist_small_signal).as_const().is_none() {
                jacobians
                    .extend(smsig_unknowns.iter().map(|unk| (residual.resist_small_signal, *unk)))
            }
            if self.cursor.func.dfg.value_def(residual.react_small_signal).as_const().is_none() {
                jacobians
                    .extend(smsig_unknowns.iter().map(|unk| (residual.react_small_signal, *unk)))
            }
        }
        jacobians
    }

    fn build_jacobian(
        &mut self,
        sim_unknown_reads: &[(ParamKind, Value)],
        derivative_info: &KnownDerivatives,
        // mapping from derivative operands to result SSA value
        derivatives: &AHashMap<(Value, Unknown), Value>,
    ) {
        let num_sim_unknowns = self.dae.unknowns.len();
        self.dae.jacobian = TiVec::with_capacity(usize::saturating_pow(num_sim_unknowns, 2));

        let deriv_unknowns = &derivative_info.unknowns;

        // each row pertains to a simulation unknown and contains Value pair (resistive, reactive)
        let mut dense_row = TiVec::from(vec![(F_ZERO, F_ZERO); num_sim_unknowns]);

        // routine to insert instructions to add/substract derivative result to
        // SSA values corresponding to each Jacobi matrix entry
        let mut add_jacobi = |matrix_entry: &mut Value, residual, unknown, negate| {
            if let Some(deriv) = derivatives.get(&(residual, unknown)).copied() {
                util::add(&mut self.cursor, matrix_entry, deriv, negate)
            }
        };

        // construct the matrix by creating a dense row first, and then sparsifying
        for (row, residual) in self.dae.residual.iter_enumerated() {
            let mut construct = |kind: SimUnknownKind, val, neg| {
                let Some(sim_unknown) = self.dae.unknowns.index_of(&kind) else { return };
                let (jac_resist, jac_react) = &mut dense_row[sim_unknown];

                if let Some(lim_vals) = self.intern.lim_state.raw.get(&val) {
                    // deal with limit state
                    for (val, negate_lim) in lim_vals {
                        let Some(lim_unknown) = deriv_unknowns.index_of(val) else { continue };
                        let neg = neg != *negate_lim;
                        add_jacobi(jac_resist, residual.resist, lim_unknown, neg);
                        add_jacobi(jac_resist, residual.resist_small_signal, lim_unknown, neg);
                        add_jacobi(jac_react, residual.react, lim_unknown, neg);
                        add_jacobi(jac_react, residual.react_small_signal, lim_unknown, neg);
                    }
                }

                if let Some(unknown) = deriv_unknowns.index_of(&val) {
                    add_jacobi(jac_resist, residual.resist, unknown, neg);
                    add_jacobi(jac_resist, residual.resist_small_signal, unknown, neg);
                    add_jacobi(jac_react, residual.react, unknown, neg);
                    add_jacobi(jac_react, residual.react_small_signal, unknown, neg);
                }
            };

            // construct the dense row
            for &(param, val) in sim_unknown_reads {
                let kind = match param {
                    ParamKind::Potential { hi, lo } => {
                        if let Some(lo) = lo {
                            construct(SimUnknownKind::KirchhoffLaw(lo), val, NEG);
                        }
                        SimUnknownKind::KirchhoffLaw(hi)
                    }
                    ParamKind::ImplicitUnknown(equation) => SimUnknownKind::Implicit(equation),
                    ParamKind::Flow(kind) => SimUnknownKind::BranchFlow(kind),
                    _ => continue,
                };
                construct(kind, val, POS);
            }

            // sparsification
            for (col, (resist, react)) in dense_row.iter_mut_enumerated() {
                if *resist == F_ZERO && *react == F_ZERO {
                    continue;
                }
                self.dae.jacobian.push(MatrixEntry {
                    row,
                    col,
                    resist: mem::replace(resist, F_ZERO),
                    react: mem::replace(react, F_ZERO),
                });
            }
        }
    }

    fn build_lim_rhs(
        &mut self,
        derivative_info: &KnownDerivatives,
        derivatives: AHashMap<(Value, Unknown), Value>,
    ) {
        for residual in &mut self.dae.residual {
            for (state, (unchanged, lim_vals)) in self.intern.lim_state.iter_enumerated() {
                for &(val, neg) in lim_vals {
                    let Some(unknown) = derivative_info.unknowns.index_of(&val) else { continue };
                    let changed = HirInterner::ensure_param_(
                        &mut self.intern.params,
                        &mut self.cursor,
                        ParamKind::NewState(state),
                    );

                    let delta = if neg {
                        self.cursor.ins().fadd(changed, *unchanged)
                    } else {
                        self.cursor.ins().fsub(changed, *unchanged)
                    };
                    let mut add_lim_rhs = |dst, residual, residual_smsig| {
                        let mut ddx =
                            derivatives.get(&(residual, unknown)).copied().unwrap_or(F_ZERO);
                        let ddx_smsig =
                            derivatives.get(&(residual_smsig, unknown)).copied().unwrap_or(F_ZERO);
                        add(&mut self.cursor, &mut ddx, ddx_smsig, POS);
                        if ddx != F_ZERO && delta != F_ZERO {
                            let rhs = self.cursor.ins().fmul(ddx, delta);
                            add(&mut self.cursor, dst, rhs, POS);
                        }
                    };
                    add_lim_rhs(
                        &mut residual.resist_lim_rhs,
                        residual.resist,
                        residual.resist_small_signal,
                    );
                    add_lim_rhs(
                        &mut residual.react_lim_rhs,
                        residual.react,
                        residual.react_small_signal,
                    );
                }
            }
        }
    }

    /// Multiply each residual and matrix entry with mfactor and ensure its optbarrier.
    ///
    /// Refer to [LRM 6.3.6] for rules of applying mfactor.
    pub(super) fn ensure_optbarriers(&mut self) {
        let mfactor = self
            .intern
            .ensure_param(&mut self.cursor, ParamKind::ParamSysFun(ParamSysFun::mfactor));
        let mut ensure_optbarrier = |mut val, is_kirchhoff_law| {
            val = self.cursor.ins().ensure_optbarrier(val);
            if is_kirchhoff_law && val != F_ZERO {
                update_optbarrier(self.cursor.func, &mut val, |val, cursor| {
                    cursor.ins().fmul(mfactor, val)
                })
            }
            self.output_values.ensure(self.cursor.func.dfg.num_values());
            self.output_values.insert(val);
            val
        };

        // residual
        for (unknown, residual) in self.dae.residual.iter_mut_enumerated() {
            // we purposefully ignore small signal values here since they never contribute to the residual
            residual.react_small_signal = F_ZERO;
            residual.react_small_signal = F_ZERO;
            let is_kirchhoff =
                matches!(self.dae.unknowns[unknown], SimUnknownKind::KirchhoffLaw(_));
            residual.map_vals(|val| ensure_optbarrier(val, is_kirchhoff));
        }
        // JW: ?
        ensure_optbarrier(mfactor, false);

        // noises
        for noise_src in &mut self.dae.noise_sources {
            noise_src.map_vals(|val| ensure_optbarrier(val, false));
        }

        // Jacobian
        for entry in &mut self.dae.jacobian {
            let is_kirchhoff =
                matches!(self.dae.unknowns[entry.row], SimUnknownKind::KirchhoffLaw(_));
            entry.resist = ensure_optbarrier(entry.resist, is_kirchhoff);
            entry.react = ensure_optbarrier(entry.react, is_kirchhoff);
        }
    }
}
