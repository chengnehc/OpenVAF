use std::mem;

use ahash::AHashMap;
use bitset::BitSet;
use hir::{BranchWrite, CompilationDB, Node, ParamSysFun};
use hir_lower::{HirInterner, ImplicitEquation, ParamKind};
use indexmap::IndexSet;
use mir::builder::InstBuilder;
use mir::cursor::{Cursor, FuncCursor};
use mir::{
    Block, ControlFlowGraph, DominatorTree, Inst, KnownDerivatives, Unknown, Value, F_ONE, F_ZERO,
};
use mir_autodiff::auto_diff;
use typed_index_collections::TiVec;

use crate::context::Context;
use crate::noise::NoiseSource;
use crate::topology::{BranchInfo, Contribution, Noise};
use crate::util::{self, add, is_op_dependent, strip_optbarrier, update_optbarrier};
use crate::SimUnknownKind;

use super::{DaeSystem, MatrixEntry, Residual, SimUnknown};

const POS: bool = false;
const NEG: bool = true;

impl Residual {
    /// Add/subtract `val` to/from the resistive value of this residual
    fn add_unknown<const NEGATE: bool>(&mut self, mut val: Value, cursor: &mut FuncCursor) {
        val = strip_optbarrier(&cursor, val);
        util::add(cursor, &mut self.resist, val, NEGATE);
    }

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
}

pub(super) struct Builder<'a> {
    pub(super) system: DaeSystem,
    pub(super) cursor: FuncCursor<'a>,
    pub(super) db: &'a CompilationDB,
    pub(super) intern: &'a mut HirInterner,
    pub(super) cfg: &'a mut ControlFlowGraph,
    pub(super) dom_tree: &'a mut DominatorTree,
    pub(super) op_dependent_insts: &'a BitSet<Inst>,
    pub(super) output_values: &'a mut BitSet<Value>,
}

impl<'a> Builder<'a> {
    pub(super) fn new(ctx: &'a mut Context) -> Self {
        ctx.compute_outputs(false);
        let mut builder = Self {
            system: DaeSystem::default(),
            cursor: FuncCursor::new(&mut ctx.func).at_exit(),
            db: ctx.db,
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
        self.system.small_signal_params = small_signal_vals;
        self
    }

    /// Consume the builder and generate the DAE system
    pub(super) fn finish(mut self) -> DaeSystem {
        let sim_unknown_reads = self.sim_unknown_reads();
        let derivative_info = self.intern.derivative_info(&self.cursor, true);
        let extra_derivatives = self
            .jacobian_derivatives(sim_unknown_reads.iter().map(|&(_, val)| val), &derivative_info);
        // TODO(perf): incrementally update dom_tree (for switch branches) instead
        self.dom_tree.compute::<true, false>(self.cursor.func, self.cfg);
        let derivatives =
            auto_diff(&mut *self.cursor.func, self.dom_tree, &derivative_info, &extra_derivatives);
        drop(extra_derivatives);
        // auto_diff may in an unlikely case add extra bb at the end, ensure we are building everything at the end
        self.cursor.goto_exit();

        self.build_jacobian(&sim_unknown_reads, &derivative_info, &derivatives);
        self.build_lim_rhs(&derivative_info, derivatives);
        self.ensure_optbarriers();

        self.system
    }
}

macro_rules! get_residual {
    ($self: ident, $unknown: expr) => {{
        let unknown = $self.ensure_unknown($unknown);
        &mut $self.system.residual[unknown]
    }};
}

impl Builder<'_> {
    fn ensure_unknown(&mut self, unknown: SimUnknownKind) -> SimUnknown {
        let (unknown, new) = self.system.unknowns.ensure(unknown);
        if new {
            self.system.residual.push(Residual::default());
        }
        unknown
    }

    pub(super) fn build_node(&mut self, node: Node) {
        self.ensure_unknown(SimUnknownKind::KirchhoffLaw(node));
    }

    pub(super) fn build_branch(&mut self, dst: BranchWrite, contribs: &BranchInfo) {
        match contribs.is_potential {
            // I(br) <+ ...
            mir::FALSE => {
                let contrib = self.flow_contribution(contribs);
                // If I(br) is used (appears at RHS), we need an extra sim_unknown
                let requires_unknown =
                    self.intern.is_param_live(&self.cursor, &ParamKind::Flow(dst.into()));
                if requires_unknown {
                    self.add_source_equation(&contrib, contribs.flow.unknown.unwrap(), dst);
                } else {
                    self.add_kirchhoff_law(&contrib, dst);
                }
            }
            // V(br) <+ ...
            mir::TRUE => {
                // If I(br) is used (appears at RHS), we need an extra sim_unknown
                let requires_unknown =
                    self.intern.is_param_live(&self.cursor, &ParamKind::Flow(dst.into()));
                // branches only used for node collapse look like pure current
                // sources, make sure to ignore these branches
                if requires_unknown || !contribs.potential.is_trivial() {
                    let contrib = self.potential_contribution(contribs);
                    self.add_source_equation(&contrib, contribs.flow.unknown.unwrap(), dst);
                }
            }
            // switch branch
            _ => {
                // In most cases, what looks like a switch branch is just node collapse hint.
                // Make sure we don't create switch branches when they aren't needed.
                let is_op_dependent = is_op_dependent(
                    contribs.is_potential,
                    &self.cursor,
                    self.intern,
                    self.op_dependent_insts,
                );
                let requires_flow_branch_unknown =
                    !self.cursor.as_ref().dfg.value_dead(contribs.flow.unknown.unwrap());

                if is_op_dependent
                    || requires_flow_branch_unknown
                    || !contribs.potential.is_trivial()
                {
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
                    let is_potential = strip_optbarrier(&self.cursor, contribs.is_potential);
                    // If condition is true, jump to potential_bb, or else go to next_block
                    self.cursor.ins().br(is_potential, potential_bb, next_block);
                    self.cursor.goto_bottom(potential_bb);
                    self.cursor.ins().jump(next_block);
                    self.cursor.goto_bottom(next_block);
                    let contrib = self.switch_branch(contribs, potential_bb, start_bb);
                    self.add_source_equation(&contrib, contribs.flow.unknown.unwrap(), dst)
                } else {
                    // Not a real switch branch
                    let contrib = self.flow_contribution(contribs);
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

    fn switch_branch(
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
                self.cursor.ins().phi(&[(flow_bb, flow_val), (potential_bb, potential_val)])
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
    /// `srcfactor` is the signal scaling factor (not power scale factor)
    /// Since power scales with mfactor, the signal scales with sqrt(mfactor)
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
    /// `srcfactor` is the signal scaling factor (not power scale factor)
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

    /// Normal nodal analysis: KCL is already enough.
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

    /// Modified nodal analysis: an extra equation/unknown for `FlowBranch` is required.
    fn add_source_equation(&mut self, contrib: &Contribution, eq_val: Value, dst: BranchWrite) {
        let residual = get_residual!(self, SimUnknownKind::FlowBranch(dst.into()));
        residual.add_contrib::<POS>(contrib, &mut self.cursor);
        residual.add_unknown::<NEG>(contrib.unknown.unwrap(), &mut self.cursor);
        self.add_noise(&contrib.noises, SimUnknownKind::FlowBranch(dst.into()), None);

        let (hi, lo) = dst.node_pair(self.db);
        let hi = SimUnknownKind::KirchhoffLaw(hi);
        let lo = lo.map(SimUnknownKind::KirchhoffLaw);
        get_residual!(self, hi).add_unknown::<POS>(eq_val, &mut self.cursor);
        if let Some(lo) = lo {
            get_residual!(self, lo).add_unknown::<NEG>(eq_val, &mut self.cursor);
        }
    }

    fn add_noise(&mut self, noises: &[Noise], hi: SimUnknownKind, lo: Option<SimUnknownKind>) {
        let hi = self.ensure_unknown(hi);
        let lo = lo.map(|lo| self.ensure_unknown(lo));
        self.system.noise_sources.extend(noises.iter().map(|src| {
            let factor = src.factor;
            NoiseSource { name: src.name, kind: src.kind.clone(), hi, lo, factor }
        }))
    }

    /// Return a list of all parameters that read from one of the simulation unknowns
    /// and therefore need to be considered during matrix construction.
    ///
    /// These should be constructed from the list of parameters instead of the list of
    /// sim_unknowns because voltage probes access two node voltages at the same time:
    ///
    /// V(x, y) = V(x) - V(y)
    ///
    /// We derive by these voltage differences to reduce the number of generated derivatives.
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

    fn build_jacobian(
        &mut self,
        sim_unknown_reads: &[(ParamKind, Value)],
        derivative_info: &KnownDerivatives,
        derivatives: &AHashMap<(Value, Unknown), Value>,
    ) {
        self.system.jacobian =
            TiVec::with_capacity(self.system.unknowns.len() * self.system.unknowns.len());

        //  construct the matrix by creating a dense row and then sparsifying
        let mut dense_row = TiVec::from(vec![(F_ZERO, F_ZERO); self.system.unknowns.len()]);
        let mut add = |matrix_entry: &mut Value, residual, unknown, negate| {
            if let Some(ddx) = derivatives.get(&(residual, unknown)).copied() {
                add(&mut self.cursor, matrix_entry, ddx, negate)
            }
        };

        for (row, residual) in self.system.residual.iter_enumerated() {
            // construct the dense row
            let mut add_residual = |sim_unknown: SimUnknownKind, unknown, negate| {
                let Some(sim_unknown) = self.system.unknowns.index(&sim_unknown) else { return };
                let (resist, react) = &mut dense_row[sim_unknown];
                if let Some(lim_vals) = self.intern.lim_state.raw.get(&unknown) {
                    for (val, negate_lim) in lim_vals {
                        let Some(lim_unknown) = derivative_info.unknowns.index(val) else {
                            continue;
                        };
                        add(resist, residual.resist, lim_unknown, negate != *negate_lim);
                        add(
                            resist,
                            residual.resist_small_signal,
                            lim_unknown,
                            negate != *negate_lim,
                        );
                        add(react, residual.react, lim_unknown, negate != *negate_lim);
                        add(react, residual.react_small_signal, lim_unknown, negate != *negate_lim);
                    }
                }

                if let Some(unknown) = derivative_info.unknowns.index(&unknown) {
                    add(resist, residual.resist, unknown, negate);
                    add(resist, residual.resist_small_signal, unknown, negate);
                    add(react, residual.react, unknown, negate);
                    add(react, residual.react_small_signal, unknown, negate);
                }
            };
            for &(kind, val) in sim_unknown_reads {
                let unknown = match kind {
                    ParamKind::Potential { hi, lo } => {
                        if let Some(lo) = lo {
                            add_residual(SimUnknownKind::KirchhoffLaw(lo), val, true);
                        }
                        SimUnknownKind::KirchhoffLaw(hi)
                    }
                    ParamKind::ImplicitUnknown(equation) => SimUnknownKind::Implicit(equation),
                    ParamKind::Flow(kind) => SimUnknownKind::FlowBranch(kind),
                    _ => continue,
                };
                add_residual(unknown, val, false);
            }

            // sparsify the row
            for (col, (resist, react)) in &mut dense_row.iter_mut_enumerated() {
                if *resist == F_ZERO && *react == F_ZERO {
                    continue;
                }
                self.system.jacobian.push(MatrixEntry {
                    row,
                    col,
                    resist: mem::replace(resist, F_ZERO),
                    react: mem::replace(react, F_ZERO),
                });
            }
        }
    }

    pub fn jacobian_derivatives(
        &self,
        simulation_unknown: impl Iterator<Item = Value>,
        derivatives: &KnownDerivatives,
    ) -> Vec<(Value, Unknown)> {
        let mut params: Vec<_> =
            simulation_unknown.filter_map(|param| derivatives.unknowns.index(&param)).collect();
        let lim_derivatives = self.intern.lim_state.raw.values().flat_map(|vals| {
            vals.iter().filter_map(|(val, _)| {
                if self.cursor.func.dfg.value_dead(*val) {
                    return None;
                }
                derivatives.unknowns.index(val)
            })
        });
        params.extend(lim_derivatives);

        let small_signal_params = self
            .system
            .small_signal_params
            .iter()
            .filter_map(|&param| derivatives.unknowns.index(&param));

        let num_unknowns = params.len() * self.system.residual.len() * 2;
        let mut res = Vec::with_capacity(num_unknowns);
        for residual in &self.system.residual {
            if self.cursor.func.dfg.value_def(residual.resist).as_const().is_none() {
                res.extend(params.iter().map(|unknown| (residual.resist, *unknown)))
            }
            if self.cursor.func.dfg.value_def(residual.react).as_const().is_none() {
                res.extend(params.iter().map(|unknown| (residual.react, *unknown)))
            }
            if self.cursor.func.dfg.value_def(residual.resist_small_signal).as_const().is_none() {
                res.extend(
                    small_signal_params
                        .clone()
                        .map(|unknown| (residual.resist_small_signal, unknown)),
                )
            }
            if self.cursor.func.dfg.value_def(residual.react_small_signal).as_const().is_none() {
                res.extend(
                    small_signal_params
                        .clone()
                        .map(|unknown| (residual.react_small_signal, unknown)),
                )
            }
        }
        res
    }

    fn build_lim_rhs(
        &mut self,
        derivative_info: &KnownDerivatives,
        derivatives: AHashMap<(Value, Unknown), Value>,
    ) {
        for residual in &mut self.system.residual {
            for (state, (unchanged, lim_vals)) in self.intern.lim_state.iter_enumerated() {
                for &(val, neg) in lim_vals {
                    let Some(unknown) = derivative_info.unknowns.index(&val) else { continue };
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
                    let mut add_lim_rhs = |dst, residual, residual_small_signal| {
                        let mut ddx =
                            derivatives.get(&(residual, unknown)).copied().unwrap_or(F_ZERO);
                        let ddx_small_signal = derivatives
                            .get(&(residual_small_signal, unknown))
                            .copied()
                            .unwrap_or(F_ZERO);
                        add(&mut self.cursor, &mut ddx, ddx_small_signal, false);
                        if ddx != F_ZERO && delta != F_ZERO {
                            let rhs = self.cursor.ins().fmul(ddx, delta);
                            add(&mut self.cursor, dst, rhs, false);
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

    /// Multiply each residual and matrix entry with mfactor and ensure it has a optbarrier
    ///
    /// Refer to [LRM 6.3.6] for rules of mfactor
    pub(super) fn ensure_optbarriers(&mut self) {
        let mfactor = self
            .intern
            .ensure_param(&mut self.cursor, ParamKind::ParamSysFun(ParamSysFun::mfactor));
        let mut ensure_optbarrier = |mut val, is_kirchoff_law| {
            val = self.cursor.ins().ensure_optbarrier(val);
            if is_kirchoff_law && val != F_ZERO {
                update_optbarrier(self.cursor.func, &mut val, |val, cursor| {
                    cursor.ins().fmul(mfactor, val)
                })
            }
            self.output_values.ensure(self.cursor.func.dfg.num_values());
            self.output_values.insert(val);
            val
        };
        for (unknown, residual) in &mut self.system.residual.iter_mut_enumerated() {
            // we purpusfully ignore small signal values here since they never contribute the residual
            residual.react_small_signal = F_ZERO;
            residual.react_small_signal = F_ZERO;
            let is_kirchoff =
                matches!(self.system.unknowns[unknown], SimUnknownKind::KirchhoffLaw(_));
            residual.map_vals(|val| ensure_optbarrier(val, is_kirchoff));
        }
        ensure_optbarrier(mfactor, false);

        for noise_src in &mut self.system.noise_sources {
            noise_src.map_vals(|val| ensure_optbarrier(val, false));
        }

        for entry in &mut self.system.jacobian {
            let is_kirchoff =
                matches!(self.system.unknowns[entry.row], SimUnknownKind::KirchhoffLaw(_));
            entry.resist = ensure_optbarrier(entry.resist, is_kirchoff);
            entry.react = ensure_optbarrier(entry.react, is_kirchoff);
        }
    }
}
