//! Separate the operating point independent part of a model into an initialization function

use stdx::packed_option::PackedOption;
use stdx::{impl_debug_display, impl_idx_from};

use ahash::{AHashMap, AHashSet, RandomState};
use bitset::{BitSet, SparseBitMatrix};
use hir::{CompilationDB, Type};
use hir_lower::{HirInterner, ParamKind, PlaceKind};
use indexmap::IndexMap;
use mir::builder::InstBuilder;
use mir::cursor::{Cursor, FuncCursor};
use mir::{
    Block, ControlFlowGraph, DominatorTree, FuncRef, Function, Inst, InstructionData, Opcode,
    Value, FALSE,
};
use mir_opt::{ClassId, GVN};
use typed_indexmap::TiMap;

use crate::context::Context;
use crate::util::{strip_optbarrier, strip_optbarrier_if_const};

#[cfg(test)]
mod tests;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub struct CacheSlot(pub u32);
impl_idx_from!(CacheSlot(u32));
impl_debug_display! {match CacheSlot{CacheSlot(id) => "cslot{id}";}}

#[derive(Debug)]
pub struct Initialization {
    pub func: Function,
    pub intern: HirInterner,

    /// The part of the model that is operating point independent and can be
    /// computed at the start of the simulation and cached afterwards
    pub cached_vals: IndexMap<Value, CacheSlot, RandomState>,

    /// To reduce the number of cache slots required, we must take GVN into
    /// account. Each slot stands for a group of values (expressions) under
    /// a specific `ClassId`, rather than a single value
    pub cache_slots: TiMap<CacheSlot, (PackedOption<ClassId>, u32), hir::Type>,
}

impl Initialization {
    pub(super) fn new(ctx: &mut Context<'_>, gvn: &GVN) -> Initialization {
        let mut builder = Builder::new(ctx);

        let mut blocks = builder.func.layout.block_cursor();
        while let Some(bb) = blocks.next(&builder.func.layout) {
            builder.init.func.layout.append_block(bb);
            builder.split_block(bb);
        }

        builder.build_init_intern();
        builder.build_init_cache(gvn);
        builder.optimize();

        builder.init
    }
}

struct Builder<'a> {
    init: Initialization,

    // mutates
    func: &'a mut Function,
    intern: &'a mut HirInterner,
    cfg: &'a mut ControlFlowGraph,
    dom_tree: &'a mut DominatorTree,

    // reads
    db: &'a CompilationDB,
    output_values: &'a BitSet<Value>,
    op_dependent_insts: &'a BitSet<Inst>,

    // temporary state
    /// mapping from eval func result value to instruction
    init_cache: IndexMap<Value, Inst, RandomState>,
    /// mapping from eval func value to init func value id
    val_map: AHashMap<Value, Value>,

    collapse_implicit: AHashSet<Value>,
    control_dep: SparseBitMatrix<Block, Block>,
}

impl<'a> Builder<'a> {
    fn new(ctx: &'a mut Context) -> Builder<'a> {
        let mut init_func = Function::new().with_name(format!("{}_init", &ctx.func.name));
        init_func.layout.make_blocks(ctx.func.layout.num_blocks());

        Builder {
            init: Initialization {
                func: init_func,
                intern: HirInterner::default(),
                cached_vals: IndexMap::with_capacity_and_hasher(128, RandomState::new()),
                cache_slots: TiMap::default(),
            },
            func: &mut ctx.func,
            intern: &mut ctx.intern,
            cfg: &mut ctx.cfg,
            dom_tree: &mut ctx.dom_tree,
            db: ctx.db,
            output_values: &ctx.output_values,
            op_dependent_insts: &ctx.op_dependent_insts,
            init_cache: IndexMap::with_capacity_and_hasher(256, RandomState::default()),
            collapse_implicit: AHashSet::new(),
            val_map: AHashMap::with_capacity(1024),
            control_dep: SparseBitMatrix::new_square(0),
        }
    }

    // Copy op-independent instructions from the `block` of evaluation function
    // to the same `block` of initialization function.
    fn split_block(&mut self, block: Block) {
        let mut insts = self.func.layout.block_inst_cursor(block);
        while let Some(inst) = insts.next(&self.func.layout) {
            if self.op_dependent_insts.contains(inst) {
                match self.func.dfg.insts[inst] {
                    // We need to copy terminators to ensure the control structure remains intact
                    InstructionData::Branch { else_dst: destination, .. }
                    | InstructionData::Jump { destination } => {
                        FuncCursor::new(&mut self.init.func)
                            .at_bottom(block)
                            .ins()
                            .jump(destination);
                        let srcloc = self.func.srclocs.get(inst).copied().unwrap_or_default();
                        self.init.func.srclocs.push(srcloc);
                    }
                    // Some callbacks have no meaning if op-dependent, especially, collapse hints:
                    // a model's topology must not be op-dependent.
                    // TODO(JW): make this a switch branch or a complie error
                    InstructionData::Call { func_ref, .. }
                        if self.intern.callbacks[func_ref].ignore_if_op_dependent() =>
                    {
                        cov_mark::hit!(ignore_if_op_dependent);
                        // iteration and modification at the same time
                        self.func.dfg.zap_inst(inst);
                        self.func.layout.remove_inst(inst);
                    }
                    _ => (),
                }
            } else {
                // only copy op-independent instructions to the new block
                self.copy_instruction(inst, block)
            }
        }
    }

    /// Copy op-independent instructions to the block of new function.
    fn copy_instruction(&mut self, inst: Inst, bb: Block) {
        let mut inst_data = self.func.dfg.insts[inst].deep_clone(
            &self.func.dfg.insts.value_lists,
            &self.func.dfg.phi_forest,
            &mut self.init.func.dfg.insts.value_lists,
            &mut self.init.func.dfg.phi_forest,
        );
        if let InstructionData::Call { func_ref, .. } = &mut inst_data {
            *func_ref = self.copy_callback(*func_ref)
        }

        let mut map_val = |val| {
            *self.val_map.entry(val).or_insert_with(|| {
                if let Some(val) = self.func.dfg.value_def(val).as_const() {
                    self.init.func.dfg.values.make_const(val)
                } else {
                    self.init.func.dfg.values.make_invalid()
                }
            })
        };
        for &val in self.func.dfg.inst_results(inst) {
            map_val(val);
        }
        for val in inst_data.arguments_mut(&mut self.init.func.dfg.insts.value_lists) {
            *val = map_val(*val);
        }

        let new_inst = self.init.func.dfg.make_inst(inst_data);
        self.init.func.dfg.make_inst_results_reusing(
            new_inst,
            self.func.dfg.inst_results(inst).iter().map(|val| Some(self.val_map[val])),
        );
        self.init.func.layout.append_inst_to_block(new_inst, bb);

        let srcloc = self.func.srclocs.get(inst).copied().unwrap_or_default();
        let new_inst_ = self.init.func.srclocs.push_and_get_key(srcloc);
        debug_assert_eq!(new_inst_, new_inst);

        // Cache instructions that:
        // 1. produce an output
        // 2. is tagged, i.e., writes to a user-specified variable
        if self.func.dfg.insts[inst].opcode() == Opcode::OptBarrier
            && self.output_values.contains(self.func.dfg.first_result(inst))
        {
            // case 1: instruction is optbarrier and its result is in the required
            // output_values
            //
            // outputs receive special treatment as we always want to cache them
            // if possible (but outputs may not be places). There are also special
            // considerations required for optbarriers.
            cov_mark::hit!(op_independent_output);
            let val = self.func.dfg.first_result(inst);
            let arg = strip_optbarrier(self.func, val);

            // we only cache outputsunless the argument of optbarrier instruction has
            // been cached, i.e., the variable corresponding to the argument is not tagged.
            if let Some(inst) =
                self.func.dfg.value_def(arg).inst().filter(|_| self.func.dfg.get_tag(arg).is_none())
            {
                cov_mark::hit!(cache_output);
                // needed to ensure the type is calculated correctly
                if let Some(tag) = self.func.dfg.get_tag(val) {
                    self.func.dfg.set_tag(arg, Some(tag));
                }
                // register the cached value, and make it a new parameter for the eval func
                let (idx, _) = self.init_cache.insert_full(arg, inst);
                let param = self.intern.params.len() + idx;
                self.func.dfg.values.make_param_at(param.into(), arg);
            }
        } else if self
            .func
            .dfg
            .inst_results(inst)
            .iter()
            .any(|val| self.func.dfg.get_tag(*val).is_some())
        {
            // case 2: instruction produces tagged result(s)
            // this means that instruction result corresponds to a user variable
            for &val in self.func.dfg.insts.results(inst) {
                // register the cached value, and make it a new parameter for the eval func
                let (idx, _) = self.init_cache.insert_full(val, inst);
                let param = self.intern.params.len() + idx;
                self.func.dfg.values.make_param_at(param.into(), val);
            }
            // remove the cached op-independent instruction from the eval func
            self.func.dfg.zap_inst(inst);
            self.func.layout.remove_inst(inst);
        } else if self.func.dfg.is_safe_to_remove(inst)
            && !self.func.dfg.insts[inst].is_terminator()
        {
            // special case: to remove collapse_hint callbacks
            self.func.dfg.zap_inst(inst);
            self.func.layout.remove_inst(inst);
        }
    }

    fn copy_callback(&mut self, cb: FuncRef) -> FuncRef {
        let cb = self.intern.callbacks[cb].clone();
        let signature = cb.signature();
        let (func_ref, changed) = self.init.intern.callbacks.ensure(cb);
        if changed {
            let func_ref_ = self.init.func.import_function(signature);
            debug_assert_eq!(func_ref_, func_ref);
        }
        func_ref
    }

    /// Builder the HIR interner for init function.
    fn build_init_intern(&mut self) {
        // intern function parameters
        for (&kind, val) in self.intern.params.iter() {
            if let Some(&val) = self.val_map.get(val) {
                let (param, _) = self.init.intern.params.insert_full(kind, val);
                self.init.func.dfg.values.make_param_at(param, val);
            }
        }
        // intern output place for collapsible implicit equations
        for (&kind, val) in &mut self.intern.outputs {
            let PlaceKind::CollapseImplicitEquation(eq) = kind else { continue };
            let Some(val) = val.take() else { continue };
            let val = strip_optbarrier_if_const(&self.func, val);

            let unknown = || self.intern.params.get(&ParamKind::ImplicitUnknown(eq)).unwrap();
            if val == FALSE || self.func.dfg.value_dead(*unknown()) {
                continue;
            }
            if let Some(&val) = self.val_map.get(&val) {
                self.collapse_implicit.insert(val);
                self.init.intern.outputs.insert(kind, val.into());
            }
        }
    }

    fn build_init_cache(&mut self, gvn: &GVN) {
        // Run aggressive DCE first on the main function to figure out which cached
        // values are actually used
        self.dom_tree.compute_postdom_frontiers(self.cfg, &mut self.control_dep);
        mir_opt::aggressive_dead_code_elimination(
            self.func,
            self.cfg,
            &|val, _| self.output_values.contains(val),
            &self.control_dep,
        );

        // create a cache slot for every equivalence class of values that was cached
        let mut extra_class_id = gvn.num_class();
        let mut ensure_cache_slot = |inst, idx, ty| {
            // get the equivalent class for `inst` in GVN, or create a new class otherwise.
            let equiv_class = gvn.inst_class(inst).expand().unwrap_or_else(|| {
                let class = extra_class_id.into();
                extra_class_id += 1;
                class
            });
            self.init.cache_slots.insert_full((equiv_class.into(), idx as u32), ty).0
        };

        self.init.cached_vals = self
            .init_cache
            .iter()
            .filter_map(|(&val, &old_inst)| {
                if self.func.dfg.value_dead(val)
                    && (!self.output_values.contains(val) || self.collapse_implicit.contains(&val))
                {
                    // make some other value here so there isn't an undefined parameter
                    self.func.dfg.values.fconst_at(0.0.into(), val);
                    return None;
                }

                let ty = if let Some(tag) = self.func.dfg.get_tag(val) {
                    // TODO(JW): is it always safe to unwrap here?
                    let place = self.intern.outputs.get_index(tag.into()).unwrap().0;
                    place.ty(self.db)
                } else if self.collapse_implicit.contains(&val) {
                    Type::Bool
                } else {
                    Type::Real
                };

                let idx = self
                    .func
                    .dfg
                    .inst_results(old_inst)
                    .iter()
                    .position(|&res| res == val)
                    .unwrap();

                let cache_slot = ensure_cache_slot(old_inst, idx, ty);

                // TODO(JW): this seems duplicate
                let param = usize::from(cache_slot) + self.intern.params.len();
                // dbg!(param);
                self.func.dfg.values.make_param_at(param.into(), val);

                let new_val = self.val_map[&val];
                let new_inst = self.init.func.dfg.value_def(new_val).unwrap_inst();
                // add optbarrier for new value in init func
                let val = FuncCursor::new(&mut self.init.func)
                    .after_inst_no_phi(new_inst)
                    .ins()
                    .ensure_optbarrier(new_val);

                Some((val, cache_slot))
            })
            .collect();
    }

    fn optimize(&mut self) {
        // first, simplify CFG of the eval function
        mir_opt::simplify_cfg(self.func, self.cfg);

        // then, optimize init function
        self.cfg.compute(&self.init.func);
        mir_opt::aggressive_dead_code_elimination(
            &mut self.init.func,
            self.cfg,
            &|val, _| {
                self.init.cached_vals.contains_key(&val) || self.collapse_implicit.contains(&val)
            },
            &self.control_dep,
        );
        mir_opt::simplify_cfg(&mut self.init.func, self.cfg);
    }
}
