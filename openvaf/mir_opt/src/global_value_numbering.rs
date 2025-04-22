//! Value numbering is a technique of determining when two computations in a program
//! are equivalent and eliminating one of them with a semantics-preserving optimization.
//!
//! MIR may contain repeated and/or redundant computations. The objective of this pass
//! is to detect such redundancies and re-use the already-computed result when possible.
//!
//! The GVN pass is typically run after other optimization passes, such as instruction
//! simplification and dead code elimination, to make it more effective. It is also
//! run in multiple iterations, to improve the quality of the optimization and to handle
//! loops and other control flow structures.
//!
//! See also: https://doc.rust-lang.org/beta/nightly-rustc/rustc_mir_transform/gvn/index.html

use std::cmp::Ordering;
use std::ops::{Index, IndexMut};
use stdx::impl_idx_math_from;
use stdx::packed_option::PackedOption;

use ahash::RandomState;
use bitset::{BitSet, HybridBitSet};
use hashbrown::raw::RawTable;
use mir::{DominatorTree, Function, Inst, Opcode, Value, ValueDef};
use typed_index_collections::TiVec;

mod expression;
#[cfg(test)]
mod tests;

use expression::{ExprResult, GVNExpression};

#[derive(Default)]
pub struct GVN {
    dfs_map: DFSMapping,
    class_map: ClassMap,

    touched_insts: BitSet<DFSId>,
    // the group of instructions whose leader were changed
    leader_changes: HybridBitSet<DFSId>,
}

impl GVN {
    pub fn init(&mut self, func: &Function, dom_tree: &DominatorTree, num_params: u32) {
        self.dfs_map.populate(func, dom_tree, num_params);
        self.class_map.init(func);
        // initially, mark all instructions touched
        self.touched_insts.set_size_enable(self.dfs_map.dfs_to_inst.len());
    }

    pub fn clear(&mut self, func: &mut Function) {
        self.class_map.clear(func);
        self.dfs_map.clear();
        self.leader_changes.clear();
    }

    pub fn inst_class(&self, inst: Inst) -> PackedOption<ClassId> {
        self.class_map.inst_class[inst]
    }

    pub fn num_class(&self) -> usize {
        self.class_map.classes.len()
    }

    pub fn solve(&mut self, func: &mut Function) {
        loop {
            let mut changed = false;
            for dfs_id in self.dfs_map.dfs_to_inst.keys() {
                if self.touched_insts.remove(dfs_id) {
                    changed = true;
                    let inst = self.dfs_map[dfs_id];
                    self.process_inst(func, inst);
                }
            }
            if !changed {
                debug_assert!(self.touched_insts.is_empty());
                break;
            }
        }
    }

    pub fn remove_unnecessary_insts(&mut self, func: &mut Function, dom_tree: &DominatorTree) {
        for class in self.class_map.classes.iter_mut() {
            // special treatment for constants and parameters
            if class.expr.opcode == Opcode::OptBarrier {
                let const_val = class.expr.payload.default().val1;
                // const propagation
                for dfs_id in class.members.iter() {
                    let inst = self.dfs_map[dfs_id];
                    let val = func.dfg.first_result(inst);
                    func.dfg.replace_uses(val, const_val);
                }
                class.members.clear();
                continue;
            }

            // do not touch classes with 0 or 1 member
            if matches!(&class.members , HybridBitSet::Sparse(insts) if insts.len() < 2) {
                continue;
            }

            // basic idea: if an leader instruction that is equivalent to `inst`
            // also dominates `inst`, `inst` can be removed.
            let mut leader = None;
            let mut members = HybridBitSet::new_empty();
            for dfs_id in class.members.iter() {
                let inst = self.dfs_map[dfs_id];

                if let Some(leader_inst) = leader {
                    let block = func.layout.inst_block(inst).unwrap();
                    let dominator = func.layout.inst_block(leader_inst).unwrap();
                    if dom_tree.dominates(block, dominator) {
                        let dst = func.dfg.first_result(inst);
                        let src = func.dfg.first_result(leader_inst);
                        func.dfg.replace_uses(dst, src);
                        func.dfg.zap_inst(inst);
                        func.layout.remove_inst(inst);
                        continue;
                    }
                }
                members.insert(dfs_id, self.dfs_map.dfs_to_inst.len());
                leader = Some(inst);
            }
            class.members = members
        }
    }

    fn process_inst(&mut self, func: &mut Function, inst: Inst) {
        if let Some(res) = ExprResult::from_inst(inst, self, func) {
            let eclass = res.into_class(inst, self, func);
            self.update_congruence_class(func, inst, eclass)
        }
    }

    fn update_congruence_class(&mut self, func: &mut Function, inst: Inst, eclass: ClassId) {
        let iclass = self.class_map.inst_class[inst];

        let class_changed = iclass != eclass.into();
        if class_changed {
            // move the instruction into new class
            self.class_map.inst_class[inst] = eclass.into();
            // update the information of the old and new class
            self.update_class_info(func, inst, iclass, eclass);
        }

        let dfs_id = self.dfs_map[inst].unwrap_unchecked();
        let leader_changed = self.leader_changes.remove(dfs_id);

        // either the class of `inst` has changed, or the leader of
        // the class to which `inst` belongs has changed, update the
        // touched instructions.
        if leader_changed || class_changed {
            for use_ in func.dfg.inst_uses(inst) {
                let inst = func.dfg.use_to_user(use_);
                let dfs_id = self.dfs_map[inst].unwrap_unchecked();
                self.touched_insts.insert(dfs_id);
            }
        }
    }

    fn update_class_info(
        &mut self,
        func: &mut Function,
        inst: Inst,
        old_class: PackedOption<ClassId>,
        new_class: ClassId,
    ) {
        let dom_size = self.dfs_map.dfs_to_inst.len();
        let dfs_id = self.dfs_map[inst].unwrap_unchecked();

        // update members and next leader of `new_class`
        // (leader has already been handled by `insert_expr()`)
        let new_class = &mut self.class_map[new_class];
        new_class.members.insert(dfs_id, dom_size);
        if new_class.leader != inst.into() {
            new_class.add_possible_next_leader(inst, dfs_id);
        }

        // if `inst` is in some old class, update its members,
        // leader and next leader.
        if let Some(id) = old_class.expand() {
            let old_class = &mut self.class_map[id];

            old_class.members.remove(dfs_id);

            if old_class.members.is_empty() {
                // `inst` is the only member of old class
                old_class.leader = None.into();
                self.class_map.remove_expr_class(id, func);
            } else if old_class.next_leader.0 == inst.into() {
                // `inst` is the next leader of the old class
                old_class.reset_next_leader();
            } else if old_class.leader == inst.into() {
                // `inst` is the leader of the old class
                let new_leader = match old_class.next_leader.0.expand() {
                    Some(next_leader) => next_leader,
                    None => {
                        let dfs_id = old_class.members.iter().max().unwrap();
                        self.dfs_map[dfs_id]
                    }
                };
                old_class.leader = new_leader.into();
                old_class.reset_next_leader();
                // mark other member instructions of the old class as 'touched',
                // which means they need to be processed in the next iteration.
                self.touched_insts.union(&old_class.members);
                // mark other member instructions of the old class as leader changed
                self.leader_changes.union(&old_class.members, dom_size);
            }
        }
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, PartialOrd, Ord, Debug)]
struct DFSId(u32);
impl_idx_math_from!(DFSId(u32));

/// DFS numbering of instructions, calculated by using reverse post-order
/// traversal of CFG.
#[derive(Default)]
struct DFSMapping {
    dfs_to_inst: TiVec<DFSId, Inst>,
    inst_to_dfs: TiVec<Inst, PackedOption<DFSId>>, // None means instruction is dead
    param_off: u32,                                // rank number offset caused by parameter
}

impl DFSMapping {
    fn populate(&mut self, func: &Function, dom_tree: &DominatorTree, num_params: u32) {
        self.param_off = num_params + 1;
        self.inst_to_dfs.resize(func.dfg.num_insts(), None.into());
        for &bb in dom_tree.cfg_postorder().iter().rev() {
            for inst in func.layout.block_insts(bb) {
                let dfs_id = self.dfs_to_inst.push_and_get_key(inst);
                self.inst_to_dfs[inst] = dfs_id.into();
            }
        }
    }

    fn clear(&mut self) {
        self.inst_to_dfs.clear();
        self.dfs_to_inst.clear();
    }

    /// Should the two operand value of a commutative binary instruction be swapped,
    /// according to the rank computed by GVN?
    fn should_swap(&self, val1: Value, val2: Value, func: &Function) -> bool {
        let rank1 = self.get_rank(val1, func);
        let rank2 = self.get_rank(val2, func);
        match rank1.cmp(&rank2) {
            Ordering::Equal => val1 > val2,
            Ordering::Less => false,
            Ordering::Greater => true,
        }
    }

    fn get_rank(&self, val: Value, func: &Function) -> u32 {
        match func.dfg.value_def(val) {
            ValueDef::Result(inst, _) => match self.inst_to_dfs[inst].expand() {
                Some(dfs) => u32::from(dfs) + self.param_off,
                None => u32::MAX,
            },
            ValueDef::Param(param) => u32::from(param) + 1,
            ValueDef::Const(_) | ValueDef::Invalid => 0,
        }
    }
}

impl Index<DFSId> for DFSMapping {
    type Output = Inst;

    fn index(&self, index: DFSId) -> &Self::Output {
        &self.dfs_to_inst[index]
    }
}

impl Index<Inst> for DFSMapping {
    type Output = PackedOption<DFSId>;

    fn index(&self, inst: Inst) -> &Self::Output {
        &self.inst_to_dfs[inst]
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Hash, PartialOrd, Ord, Debug)]
pub struct ClassId(u32);
impl_idx_math_from!(ClassId(u32));

#[derive(Default)]
struct ClassMap {
    classes: TiVec<ClassId, EquivalenceClass>,
    inst_class: TiVec<Inst, PackedOption<ClassId>>,
    // TODO(JW): switch to safe `HashTable` API
    /// Hash table for expression class
    expr_class: RawTable<ClassId>,
    state: RandomState,
}

impl ClassMap {
    fn init(&mut self, func: &Function) {
        self.inst_class.resize(func.dfg.num_insts(), None.into());
        self.expr_class.reserve(func.dfg.num_insts(), |_| unreachable!());
    }

    fn remove_expr_class(&mut self, class: ClassId, func: &mut Function) {
        // self.classes[class].members.clear();
        // debug_assert!(self.classes[class].members.is_empty());
        let hash = self.classes[class].expr.hash(&self.state, func);
        self.expr_class.remove_entry(hash, |it| *it == class);
        self.classes[class].expr.destroy(func)
    }

    fn insert_expr(&mut self, inst: Inst, mut expr: GVNExpression, func: &mut Function) -> ClassId {
        let hash = expr.hash(&self.state, func);
        if let Some(class) =
            self.expr_class.get(hash, |class| self.classes[*class].expr.eq(&expr, func))
        {
            expr.destroy(func);
            *class
        } else {
            let new_class = EquivalenceClass {
                expr,
                leader: inst.into(),
                next_leader: (None.into(), u32::MAX.into()),
                members: HybridBitSet::new_empty(),
            };
            let new_class = self.classes.push_and_get_key(new_class);
            unsafe { self.expr_class.insert_no_grow(hash, new_class) };

            new_class
        }
    }

    /// Get the leading value that is semantically equivalent to `val`.
    fn get_lead_val(&self, val: Value, func: &Function) -> Value {
        if let ValueDef::Result(inst, _) = func.dfg.value_def(val) {
            if let Some(id) = self.inst_class[inst].expand() {
                let class = &self.classes[id];
                if class.expr.opcode == Opcode::OptBarrier {
                    return class.expr.payload.default().val1;
                } else {
                    return func.dfg.first_result(class.leader.unwrap_unchecked());
                }
            }
        }
        val
    }

    fn clear(&mut self, func: &mut Function) {
        for class in self.classes.iter_mut() {
            class.expr.destroy(func)
        }
        self.inst_class.clear();
        self.expr_class.clear_no_drop();
        self.classes.clear();
    }
}

impl Index<ClassId> for ClassMap {
    type Output = EquivalenceClass;

    fn index(&self, index: ClassId) -> &Self::Output {
        &self.classes[index]
    }
}

impl IndexMut<ClassId> for ClassMap {
    fn index_mut(&mut self, index: ClassId) -> &mut Self::Output {
        &mut self.classes[index]
    }
}

struct EquivalenceClass {
    expr: GVNExpression,
    /// the leader instruction in this equivalence expression class
    leader: PackedOption<Inst>,
    /// the possible next leader instruction
    next_leader: (PackedOption<Inst>, DFSId),
    /// member instructions under this equivalence class, represented by their `DFSId`
    members: HybridBitSet<DFSId>,
}

impl EquivalenceClass {
    pub fn reset_next_leader(&mut self) {
        self.next_leader = (None.into(), u32::MAX.into())
    }

    pub fn add_possible_next_leader(&mut self, inst: Inst, dfs_num: DFSId) {
        if dfs_num < self.next_leader.1 {
            self.next_leader = (inst.into(), dfs_num);
        }
    }
}
