use stdx::packed_option::PackedOption;

use bitset::{BitSet, SparseBitMatrix};
use hir::{CompilationDB, ModuleInfo};
use hir_lower::{HirInterner, MirBuilder, PlaceKind};
use lasso::Rodeo;
use mir::{ControlFlowGraph, DominatorTree, Function, Inst, Value};
use mir_opt::{
    aggressive_dead_code_elimination, global_value_numbering, inst_combine, propagate_direct_taint,
    propagate_taint, simplify_cfg, simplify_cfg_no_phi_merge,
    sparse_conditional_constant_propagation, standard_dead_code_elimination, GVN,
};

pub(crate) struct Context<'a> {
    pub(crate) db: &'a CompilationDB,
    pub(crate) module: &'a ModuleInfo,
    pub(crate) func: Function,
    pub(crate) intern: HirInterner,
    pub(crate) cfg: ControlFlowGraph,
    pub(crate) dom_tree: DominatorTree,
    pub(crate) output_values: BitSet<Value>,
    pub(crate) op_dependent_insts: BitSet<Inst>,
    pub(crate) op_dependent_vals: Vec<Value>,
}

impl<'a> Context<'a> {
    pub fn new(db: &'a CompilationDB, literals: &mut Rodeo, module: &'a ModuleInfo) -> Self {
        let is_output = |kind| match kind {
            PlaceKind::Contribute { .. }
            | PlaceKind::IsPotential(_)
            | PlaceKind::ImplicitResidual { .. }
            | PlaceKind::CollapseImplicitEquation(_) => true,
            PlaceKind::Var(var) => module.op_vars.contains_key(&var),
            _ => false,
        };

        // for simulator backend, we need to lower equations and tag the writes
        // in order to support function splitting and parameter caching.
        let (mut func, mut intern) =
            MirBuilder::new(db, module.module, &is_output, &mut module.op_vars.keys().copied())
                .with_equations()
                .with_write_tags()
                .build(literals);

        // std::fs::write(format!("/tmp/{}.mir-raw", module.module.name(db)), func.to_debug_string())
        //   .unwrap();
        //
        // dbg!(&intern.params);

        intern.insert_var_init(db, &mut func, literals); // TODO hidden state

        Context {
            db,
            module,
            func,
            intern,
            cfg: ControlFlowGraph::default(),
            dom_tree: DominatorTree::default(),
            output_values: BitSet::default(),
            op_dependent_insts: BitSet::default(),
            op_dependent_vals: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptimizationStage {
    Initial,
    PostDerivative,
    Final,
}

impl Context<'_> {
    #[inline]
    pub fn compute_cfg(&mut self) {
        self.cfg.compute(&self.func);
    }

    /// Compute the set of output values according to HIR interner.
    /// We need this information to make optbarriers and control the behavior of some optimization passes.
    ///
    /// # Note
    /// This will clear and overwrite any information already stored
    pub fn compute_outputs<const WITH_CONTRIB: bool>(&mut self) {
        self.output_values.clear();
        self.output_values.ensure(self.func.dfg.num_values() + 1);
        if WITH_CONTRIB {
            self.output_values
                .extend(self.intern.outputs.values().copied().filter_map(PackedOption::expand));
        } else {
            // filter out contributions
            for (kind, val) in self.intern.outputs.iter() {
                if matches!(kind, PlaceKind::Var(var) if self.module.op_vars.contains_key(var))
                    || matches!(kind, PlaceKind::CollapseImplicitEquation(_) | PlaceKind::BoundStep)
                {
                    self.output_values.insert(val.unwrap_unchecked());
                }
            }
        }
    }

    /// Run pre-defined optimization passes at different compile stages.
    pub fn optimize(&mut self, stage: OptimizationStage) -> GVN {
        let Self { func, cfg, .. } = self;

        if stage == OptimizationStage::Initial {
            standard_dead_code_elimination(func, &self.output_values);
        }

        sparse_conditional_constant_propagation(func, cfg);

        inst_combine(func);

        if stage == OptimizationStage::Final {
            simplify_cfg(func, cfg);
        } else {
            simplify_cfg_no_phi_merge(func, cfg);
        }

        self.dom_tree = DominatorTree::with_func_and_cfg::<true, true>(func, cfg);
        let gvn = global_value_numbering(func, &self.dom_tree, self.intern.params.len() as u32);

        if stage == OptimizationStage::Final {
            let mut control_dep = SparseBitMatrix::new_square(0);
            self.dom_tree.compute_postdom_frontiers(cfg, &mut control_dep);

            aggressive_dead_code_elimination(
                func,
                cfg,
                &|val, _| self.output_values.contains(val),
                &control_dep,
            );

            simplify_cfg(func, cfg);
        }

        gvn
    }

    /// Get op-dependent instructions for topology setup
    pub fn init_op_dependent_insts(&mut self) {
        let mut dom_frontiers = SparseBitMatrix::new_square(self.func.layout.num_blocks());
        self.dom_tree.compute_dom_frontiers(&self.cfg, &mut dom_frontiers);

        let dfg = &mut self.func.dfg;
        self.op_dependent_insts.ensure(dfg.num_insts());

        // Deal with noise sources
        for (cb, insts) in self.intern.callback_callers.iter_mut_enumerated() {
            if self.intern.callbacks[cb].is_noise() {
                insts.retain(|&inst| {
                    if self.func.layout.inst_block(inst).is_some() {
                        self.op_dependent_insts.insert(inst);
                        self.op_dependent_vals.extend(dfg.inst_results(inst));
                        true
                    } else {
                        false
                    }
                })
            }
        }
        // Prepare taint source: live, op-dependent parameters
        for (param, &val) in self.intern.params.iter() {
            if !dfg.value_dead(val) && param.is_op_dependent() {
                self.op_dependent_vals.push(val)
            }
        }

        propagate_direct_taint(
            &self.func,
            &dom_frontiers,
            self.op_dependent_vals.iter().copied(),
            &mut self.op_dependent_insts,
        );
    }

    /// Get full op-dependent instructions for function split
    pub fn refresh_op_dependent_insts(&mut self) {
        let dfg = &mut self.func.dfg;
        self.op_dependent_vals.clear();
        self.op_dependent_insts.clear();
        self.op_dependent_insts.ensure(dfg.num_insts());
        for (cb, insts) in self.intern.callback_callers.iter_mut_enumerated() {
            if self.intern.callbacks[cb].is_op_dependent() {
                insts.retain(|&inst| {
                    if self.func.layout.inst_block(inst).is_some() {
                        self.op_dependent_insts.insert(inst);
                        self.op_dependent_vals.extend(dfg.inst_results(inst));
                        true
                    } else {
                        false
                    }
                })
            }
        }
        for (param, &val) in self.intern.params.iter() {
            if !dfg.value_dead(val) && param.is_op_dependent() {
                self.op_dependent_vals.push(val)
            }
        }

        propagate_taint(
            &self.func,
            &self.cfg,
            &self.dom_tree,
            self.op_dependent_vals.iter().copied(),
            &mut self.op_dependent_insts,
        );
    }
}
