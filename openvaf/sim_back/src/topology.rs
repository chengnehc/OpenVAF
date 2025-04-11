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
mod lineralize;
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
    pub is_potential: Value,

    /// Contribution to potential source branch destination
    ///
    /// This should normally not be used by compact models, except for node collapse.
    pub potential: Contribution,

    /// Contribution to flow source branch destination
    ///
    /// This should be the primary kind of contributions for most compact models.
    pub flow: Contribution,
}

#[derive(Debug, Clone)]
pub(crate) struct Contribution {
    /// If the conrtibution is also probed
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
    Branch { id: BranchId, is_potential: bool, is_reactive: bool },
    Implicit { id: ImplicitEquation, is_reactive: bool },
}

impl ContributeKind {
    pub fn is_reactive(self) -> bool {
        match self {
            ContributeKind::Branch { is_reactive, .. }
            | ContributeKind::Implicit { is_reactive, .. } => is_reactive,
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

/// An intermediate representation the topology of a model. It represents
/// topology as a set of contributions to branches and implicit equations.
/// These contributions are divided into resistive/reactive voltage/current.
#[derive(Debug)]
pub(crate) struct Topology {
    pub(crate) branches: TiVec<BranchId, BranchInfo>,
    pub(crate) implicit_equations: TiVec<ImplicitEquation, Contribution>,
    pub(crate) small_signal_vals: IndexSet<Value, ahash::RandomState>,
    // a temporary map for building topology
    contrib_map: AHashMap<Value, ContributeKind>,
}

impl Topology {
    pub(crate) fn new(ctxt: &mut Context) -> Self {
        let mut branches = TiVec::with_capacity(128);
        let mut implicit_equations: TiVec<_, _> = ctxt
            .intern
            .implicit_equations
            .keys()
            .filter_map(|eq| {
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
            .collect();
        let small_signal_vals =
            IndexSet::with_capacity_and_hasher(128, ahash::RandomState::default());

        let mut contrib_map = AHashMap::with_capacity(128); // a temporary map used during building
        for (kind, val) in &ctxt.intern.outputs {
            let Some(val) = val.expand() else { continue };
            let val = strip_optbarrier_if_const(&ctxt.func, val);
            match *kind {
                PlaceKind::ImplicitResidual { equation: id, reactive: false } => {
                    implicit_equations[id].resist = val;
                    contrib_map.insert(val, ContributeKind::Implicit { id, is_reactive: false });
                }
                PlaceKind::ImplicitResidual { equation: id, reactive: true } => {
                    implicit_equations[id].react = val;
                    contrib_map.insert(val, ContributeKind::Implicit { id, is_reactive: true });
                }
                PlaceKind::IsPotential(branch) => {
                    let id = branches.next_key();
                    let (hi, lo) = branch.node_pair(ctxt.db);
                    let is_potential = val;

                    // A potential source branch cannot be described by KFL, additional
                    // branch flow simulation unknown is required.
                    let requires_unknown = is_potential != mir::FALSE;

                    // Check if the potential/flow of the branch is probed.
                    // If so, make sure that the probed value is passed as function parameter.
                    let potential_probed =
                        ctxt.intern.is_param_live(&ctxt.func, &ParamKind::Potential { hi, lo });
                    let flow_probed =
                        ctxt.intern.is_param_live(&ctxt.func, &ParamKind::Flow(branch.into()));
                    let potential_unknown = (requires_unknown || potential_probed).then(|| {
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

                    let mut get_contrib = |is_reactive, is_potential| {
                        ctxt.intern
                            .outputs
                            .get(&PlaceKind::Contribute { dst: branch, is_reactive, is_potential })
                            .and_then(|it| it.expand())
                            .map(|val| {
                                let val = strip_optbarrier_if_const(&ctxt.func, val);
                                contrib_map.insert(
                                    val,
                                    ContributeKind::Branch { id, is_potential, is_reactive },
                                );
                                val
                            })
                            .unwrap_or(F_ZERO)
                    };

                    branches.push(BranchInfo {
                        branch,
                        is_potential,
                        potential: Contribution {
                            unknown: potential_unknown,
                            resist: get_contrib(false, true),
                            react: get_contrib(true, true),
                            resist_small_signal: F_ZERO,
                            react_small_signal: F_ZERO,
                            noises: Vec::new(),
                        },
                        flow: Contribution {
                            unknown: flow_unknown,
                            resist: get_contrib(false, false),
                            react: get_contrib(true, false),
                            resist_small_signal: F_ZERO,
                            react_small_signal: F_ZERO,
                            noises: Vec::new(),
                        },
                    });
                }
                _ => {}
            }
        }

        let mut topology =
            Topology { branches, implicit_equations, small_signal_vals, contrib_map };

        let mut postdom_frontiers = SparseBitMatrix::new_square(ctxt.func.layout.num_blocks());
        ctxt.init_op_dependent_insts(&mut postdom_frontiers);
        ctxt.dom_tree.compute_postdom_frontiers(&ctxt.cfg, &mut postdom_frontiers);

        let num_insts = ctxt.func.dfg.num_insts();

        let mut builder = Builder {
            topology: &mut topology,
            db: ctxt.db,
            func: &mut ctxt.func,
            output_values: &ctxt.output_values,
            cfg: &mut ctxt.cfg,
            scratch_buf: BitSet::new_empty(num_insts),
            postorder: Vec::with_capacity(128),
            val_map: AHashMap::with_capacity(128),
            edges: Vec::with_capacity(128),
            phis: Vec::with_capacity(128),
            op_dependent_insts: &ctxt.op_dependent_insts,
            op_dependent_vals: &ctxt.op_dependent_vals,
        };

        let operators = builder.analog_operator_evaluations(&postdom_frontiers, &mut ctxt.intern);
        drop(postdom_frontiers);
        builder.builid_analog_operators(operators, &mut ctxt.intern);
        mir_opt::simplify_cfg_no_phi_merge(builder.func, builder.cfg);
        builder.prune_small_signal();

        // JW: I suppose this is a temporary field only used during building. Therefore, clearing it helps
        // reducing memory usage. For debug purposes, this field shall be retained. However, the
        // order of hashmap is arbitrary, and will cause the unit tests to fail, so this field is
        // still cleared anyway.
        //
        // if !cfg!(debug_assertions) {
        //     topology.contributes = AHashMap::new();
        // }
        //
        // clear the hashmap
        topology.contrib_map = AHashMap::new();

        topology
    }

    fn new_implicit_equation(&mut self, equation: ImplicitEquation, contrib: Contribution) {
        if contrib.resist != F_ZERO {
            self.contrib_map.insert(
                contrib.resist,
                ContributeKind::Implicit { id: equation, is_reactive: false },
            );
        }
        if contrib.react != F_ZERO {
            self.contrib_map.insert(
                contrib.react,
                ContributeKind::Implicit { id: equation, is_reactive: true },
            );
        }
        let eq = self.implicit_equations.push_and_get_key(contrib);
        debug_assert_eq!(eq, equation);
    }

    fn as_contribute_kind(&self, val: Value) -> Option<ContributeKind> {
        self.contrib_map.get(&val).copied()
    }

    fn get_contrib_mut(&mut self, kind: ContributeKind) -> &mut Contribution {
        match kind {
            ContributeKind::Branch { id, is_potential: true, .. } => {
                &mut self.branches[id].potential
            }
            ContributeKind::Branch { id, is_potential: false, .. } => &mut self.branches[id].flow,
            ContributeKind::Implicit { id, .. } => &mut self.implicit_equations[id],
        }
    }
}
