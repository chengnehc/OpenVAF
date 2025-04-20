//! This module is responsible for building a set of coherent branch
//! definitions from the raw lowering results. These can be used to
//! build the model topology (residual, matrix, noise sources)
//! without significant additional analysis.
//!
//! This process is quite involved as many things in Verilog-A are quite
//! implicit and don't quite match our topology model. In particular this
//! module will:
//!
//! * Turn function calls like `ddt` and `white_noise` either into direct
//!   contributions if possible (linearization), or into an implicit node/
//!   internal equation.
//! * Determine all nodes which are statically known to always have a large
//!   signal voltage of zero (small_signal_network).
//! * Separate contributions made from the small signal network to the large
//!   signal network into separate values where possible (prune). Avoid
//!   generating unnecessary derivatives.

use stdx::{impl_debug_display, impl_idx_from};

use ahash::AHashMap;
use bitset::{BitSet, SparseBitMatrix};
use hir::BranchWrite;
use hir_lower::{CallBackKind, HirInterner, ImplicitEquation, ParamKind, PlaceKind};
use indexmap::IndexSet;
use lasso::Spur;
use mir::{Function, Inst, Value, F_ZERO};
use mir_build::SSAVariableBuilder;
use typed_index_collections::TiVec;

use crate::context::Context;
use crate::noise::NoiseSourceKind;
use crate::util::{strip_optbarrier, strip_optbarrier_if_const};

mod builder;
mod linearize;
mod small_signal_network;
#[cfg(test)]
mod tests;

use builder::Builder;

#[derive(Debug)]
pub(crate) struct BranchInfo {
    pub branch: BranchWrite,

    /// A boolean flag of whether the contribution kind of this branch is potential
    ///
    /// For switch branch, where branch contribution is dynamically switched between
    /// potential and flow according to run-time parameters, the value is the result
    /// of phi instructions.
    pub is_potential_source: Value,

    /// Contribution to potential source branch destination
    ///
    /// This should normally not be used by compact models, except for node collapse.
    pub potential: Contribution,

    /// Contribution to flow source branch destination
    ///
    /// This should be the primary kind of contributions for most compact models.
    pub flow: Contribution,
}

impl BranchInfo {
    pub fn branch_flow_unknown(&self) -> Value {
        self.flow.unknown.unwrap()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Contribution {
    pub unknown: Option<Value>,
    pub resist: Value,
    pub react: Value,
    pub resist_small_signal: Value,
    pub react_small_signal: Value,
    pub noises: Vec<Noise>,
}

impl Contribution {
    pub fn is_trivial(&self) -> bool {
        self.resist == F_ZERO
            && self.react == F_ZERO
            && self.resist_small_signal == F_ZERO
            && self.react_small_signal == F_ZERO
            && self.noises.is_empty()
    }
}

impl Default for Contribution {
    fn default() -> Self {
        Contribution {
            unknown: None,
            resist: F_ZERO,
            react: F_ZERO,
            resist_small_signal: F_ZERO,
            react_small_signal: F_ZERO,
            noises: Vec::new(),
        }
    }
}

/// A unique ID for a branch contribution destination
#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct BranchId(u32);
impl_idx_from!(BranchId(u32));
impl_debug_display! {
    match BranchId {BranchId(id) => "branch{id}";}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContributeKind {
    Branch { id: BranchId, potential: bool, reactive: bool },
    Implicit { id: ImplicitEquation, reactive: bool },
}

impl ContributeKind {
    pub fn is_reactive(self) -> bool {
        match self {
            ContributeKind::Branch { reactive, .. } | ContributeKind::Implicit { reactive, .. } => {
                reactive
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Noise {
    pub name: Spur,
    pub kind: NoiseSourceKind,
    pub factor: Value,
}

impl Noise {
    pub fn new(
        inst: Inst,
        cb: &CallBackKind,
        factor: Value,
        ssa_builder: &mut SSAVariableBuilder,
        func: &mut Function,
    ) -> Noise {
        let (kind, name) = match *cb {
            CallBackKind::WhiteNoise { name, .. } => {
                let mut pwr = func.dfg.instr_args(inst)[0];
                pwr = ssa_builder.define_at_exit(func, F_ZERO, pwr, inst);
                (NoiseSourceKind::WhiteNoise { pwr }, name)
            }
            CallBackKind::FlickerNoise { name, .. } => {
                let pwr = func.dfg.instr_args(inst)[0];
                let exp = func.dfg.instr_args(inst)[1];
                (
                    NoiseSourceKind::FlickerNoise {
                        pwr: ssa_builder.define_at_exit(func, F_ZERO, pwr, inst),
                        exp: ssa_builder.define_at_exit(func, F_ZERO, exp, inst),
                    },
                    name,
                )
            }
            CallBackKind::NoiseTable(ref table) => (
                NoiseSourceKind::NoiseTable { log: table.log, vals: table.vals.clone() },
                table.name,
            ),
            _ => unreachable!(),
        };
        Noise { name, kind, factor }
    }
}

/// An intermediate representation for the topology of a model.
///
/// It represents topology as a set of branch contributions and implicit equations.
#[derive(Debug)]
pub(crate) struct Topology {
    pub(crate) branches: TiVec<BranchId, BranchInfo>,
    pub(crate) implicit_equations: TiVec<ImplicitEquation, Contribution>,
    pub(crate) small_signal_vals: IndexSet<Value, ahash::RandomState>,
}

impl Topology {
    pub(crate) fn new(ctxt: &mut Context) -> Self {
        let mut contrib_map = AHashMap::with_capacity(128);

        let mut branches = TiVec::with_capacity(128);
        // Collect idt() induced implicit equations first, ddt and noise
        // induced ones will be added by the topology builder.
        let mut implicit_equations = Topology::collect_idt_implicit_equations(ctxt);
        let small_signal_vals =
            IndexSet::with_capacity_and_hasher(128, ahash::RandomState::default());

        for (place, val) in ctxt.intern.outputs.iter() {
            let Some(val) = val.expand() else { continue };
            let val = strip_optbarrier_if_const(&ctxt.func, val);
            match *place {
                PlaceKind::ImplicitResidual { equation: id, reactive: false } => {
                    implicit_equations[id].resist = val;
                    contrib_map.insert(val, ContributeKind::Implicit { id, reactive: false });
                }
                PlaceKind::ImplicitResidual { equation: id, reactive: true } => {
                    implicit_equations[id].react = val;
                    contrib_map.insert(val, ContributeKind::Implicit { id, reactive: true });
                }
                PlaceKind::IsPotential(branch) => {
                    let (hi, lo) = branch.node_pair(ctxt.db);
                    let is_potential_source = val;

                    // A potential source cannot be formulated by KFL nodal analysis,
                    // additional branch flow simulation unknown is required.
                    let requires_unknown = is_potential_source != mir::FALSE;

                    // Check if the potential/flow of the branch is probed.
                    let potential_probed =
                        ctxt.intern.is_param_live(&ctxt.func, &ParamKind::Potential { hi, lo });
                    let flow_probed =
                        ctxt.intern.is_param_live(&ctxt.func, &ParamKind::Flow(branch.into()));

                    // If so, make sure that the probed value is passed as a parameter.
                    let pot_unknown = (requires_unknown || potential_probed).then(|| {
                        HirInterner::ensure_param_(
                            &mut ctxt.intern.params,
                            &mut ctxt.func,
                            ParamKind::Potential { hi, lo },
                        )
                    });
                    let flow_unknown = (requires_unknown || flow_probed).then(|| {
                        HirInterner::ensure_param_(
                            &mut ctxt.intern.params,
                            &mut ctxt.func,
                            ParamKind::Flow(branch.into()),
                        )
                    });

                    let get_contrib = |is_potential| {
                        ctxt.intern
                            .outputs
                            .get(&PlaceKind::Contribute { branch, is_potential })
                            .and_then(|it| it.expand())
                            .map_or(F_ZERO, |val| strip_optbarrier_if_const(&ctxt.func, val))
                    };
                    let pot_resist = get_contrib(true);
                    let flow_resist = get_contrib(false);

                    let id = branches.push_and_get_key(BranchInfo {
                        branch,
                        is_potential_source,
                        potential: Contribution {
                            unknown: pot_unknown,
                            resist: pot_resist,
                            ..Default::default()
                        },
                        flow: Contribution {
                            unknown: flow_unknown,
                            resist: flow_resist,
                            ..Default::default()
                        },
                    });

                    let kind = ContributeKind::Branch { id, potential: true, reactive: false };
                    contrib_map.insert(pot_resist, kind);
                    let kind = ContributeKind::Branch { id, potential: false, reactive: false };
                    contrib_map.insert(flow_resist, kind);
                }
                _ => {}
            }
        }

        let num_insts = ctxt.func.dfg.num_insts();
        let num_blocks = ctxt.func.layout.num_blocks();

        let mut builder = Builder {
            topology: Topology { branches, implicit_equations, small_signal_vals },
            func: &mut ctxt.func,
            db: ctxt.db,
            output_values: &ctxt.output_values,
            op_dependent_insts: &ctxt.op_dependent_insts,
            op_dependent_vals: &ctxt.op_dependent_vals,
            visited_set: BitSet::new_empty(num_insts),
            val_map: AHashMap::with_capacity(128),
            contrib_map,
        };
        dbg!(&builder.func, &builder.op_dependent_vals, &builder.contrib_map);

        let mut pdf = SparseBitMatrix::new_square(num_blocks);
        ctxt.dom_tree.compute_postdom_frontiers(&ctxt.cfg, &mut pdf);
        builder.build_analog_operators(&ctxt.cfg, &pdf, &mut ctxt.intern);
        drop(pdf);

        // simplify the CFG first before pruning small signal network
        mir_opt::simplify_cfg_no_phi_merge(builder.func, &mut ctxt.cfg);
        builder.prune_small_signal();

        builder.topology
    }

    fn collect_idt_implicit_equations(ctxt: &mut Context) -> TiVec<ImplicitEquation, Contribution> {
        ctxt.intern
            .implicit_equations
            .keys()
            .filter_map(|eq| {
                // filter out dead and collapsible implicit unknowns
                let &eq_val = ctxt.intern.params.get(&ParamKind::ImplicitUnknown(eq))?;
                if ctxt.func.dfg.value_dead(eq_val) {
                    return None;
                }
                let is_collapsed = ctxt
                    .intern
                    .outputs
                    .get(&PlaceKind::CollapseImplicitEquation(eq))
                    .and_then(|val| val.expand())
                    .map(|val| strip_optbarrier(&ctxt.func, val));
                if is_collapsed == Some(mir::TRUE) {
                    ctxt.func.dfg.replace_uses(eq_val, F_ZERO);
                    return None;
                }

                Some(Contribution {
                    unknown: ctxt.intern.params.get(&ParamKind::ImplicitUnknown(eq)).copied(),
                    ..Contribution::default()
                })
            })
            .collect()
    }

    fn get_contrib_mut(&mut self, kind: ContributeKind) -> &mut Contribution {
        match kind {
            ContributeKind::Branch { id, potential: true, .. } => &mut self.branches[id].potential,
            ContributeKind::Branch { id, potential: false, .. } => &mut self.branches[id].flow,
            ContributeKind::Implicit { id, .. } => &mut self.implicit_equations[id],
        }
    }
}
