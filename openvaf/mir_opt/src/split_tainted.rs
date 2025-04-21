use bitset::{BitSet, HybridBitSet, SparseBitMatrix};
use mir::{Block, ControlFlowGraph, DominatorTree, Function, Inst, InstructionData, Value};

/// By 'direct', it means that op-independent instructions within op-dependent
/// conditional blocks are not tainted.
///
/// This is meant to be used for op-dependent instruction for topology setup.
pub fn propagate_direct_taint(
    func: &Function,
    dom_frontiers: &SparseBitMatrix<Block, Block>,
    taint_source: impl Iterator<Item = Value>,
    tainted_insts: &mut BitSet<Inst>,
) {
    tainted_insts.ensure(func.dfg.num_insts());
    let mut solver =
        DirectTaintSolver { func, dom_frontiers, inst_work_stack: Vec::new(), tainted_insts };

    // Initial propagation
    for val in taint_source {
        for use_ in func.dfg.uses(val) {
            let inst = func.dfg.use_to_user(use_);
            solver.taint_inst(inst)
        }
    }
    // Recursive propagation
    solver.solve();
}

struct DirectTaintSolver<'a> {
    // Inputs
    func: &'a Function,
    dom_frontiers: &'a SparseBitMatrix<Block, Block>,
    // Work stack used during solving taint propagation
    inst_work_stack: Vec<Inst>,
    // Output
    tainted_insts: &'a mut BitSet<Inst>,
}

impl DirectTaintSolver<'_> {
    fn taint_inst(&mut self, inst: Inst) {
        if self.tainted_insts.insert(inst) {
            self.inst_work_stack.push(inst);
        }
    }

    fn taint_dom_frontier_phis(&mut self, frontiers: impl Iterator<Item = Block>) {
        for block in frontiers {
            for inst in self.func.layout.block_insts(block) {
                if self.func.dfg.insts[inst].is_phi() {
                    self.taint_inst(inst)
                } else {
                    break;
                }
            }
        }
    }

    fn solve(&mut self) {
        while let Some(inst) = self.inst_work_stack.pop() {
            if let InstructionData::Branch { then_dst, else_dst, .. } = self.func.dfg.insts[inst] {
                // dominance frontiers are blocks where control flow merges and phis should be placed.
                //
                // TODO(JW): for pure if-else statements without loops, DF(then_dst) == DF(else_dst)
                // dbg!(&self.dom_frontiers.row(then_dst), &self.dom_frontiers.row(else_dst));
                if let Some(frontiers) = self.dom_frontiers.row(then_dst) {
                    self.taint_dom_frontier_phis(frontiers.iter());
                }
                if let Some(frontiers) = self.dom_frontiers.row(else_dst) {
                    self.taint_dom_frontier_phis(frontiers.iter());
                }
            } else {
                for use_ in self.func.dfg.inst_uses(inst) {
                    let inst = self.func.dfg.use_to_user(use_);
                    self.taint_inst(inst);
                }
            }
        }
    }
}

/// Taint op-dependent instructions and op-independent instructions within op-dependent
/// conditional blocks.
pub fn propagate_taint(
    func: &Function,
    cfg: &ControlFlowGraph,
    dom_tree: &DominatorTree,
    taint_source: impl Iterator<Item = Value>,
    tainted_insts: &mut BitSet<Inst>, // Result returned through `tainted_insts` by reference.
) {
    tainted_insts.ensure(func.dfg.num_insts());
    let mut solver = TaintSolver {
        func,
        cfg,
        dom_tree,
        inst_work_stack: Vec::new(),
        block_work_stack: Vec::new(),
        tainted_blocks: BitSet::new_empty(func.layout.num_blocks()),
        tainted_insts,
    };

    for val in taint_source {
        for use_ in func.dfg.uses(val) {
            let inst = func.dfg.use_to_user(use_);
            solver.taint_inst(inst)
        }
    }

    solver.solve();
}

struct TaintSolver<'a> {
    // Inputs
    func: &'a Function,
    cfg: &'a ControlFlowGraph,
    dom_tree: &'a DominatorTree,
    // Temporary states
    inst_work_stack: Vec<Inst>,
    block_work_stack: Vec<Block>,
    tainted_blocks: BitSet<Block>,
    // Output
    tainted_insts: &'a mut BitSet<Inst>,
}

impl TaintSolver<'_> {
    fn taint_inst(&mut self, inst: Inst) {
        if self.tainted_insts.insert(inst) {
            self.inst_work_stack.push(inst);
        }
    }

    fn taint_block(&mut self, mut bb: Block, end: Option<Block>) {
        // TODO: benchmark whether permanent hashmap is faster?
        let mut visited = HybridBitSet::new_empty();
        let size = self.func.layout.num_blocks();
        loop {
            while Some(bb) != end {
                if self.tainted_blocks.insert(bb) {
                    for inst in self.func.layout.block_insts(bb) {
                        self.taint_inst(inst);
                    }
                }
                let mut successors = self.cfg.succ_iter(bb);
                // enumlate tail recursion
                let Some(succ) = successors.find(|&bb| visited.insert(bb, size)) else { break };
                bb = succ;

                for succ in successors {
                    if visited.insert(succ, size) {
                        self.block_work_stack.push(succ);
                    }
                }
            }
            let Some(next) = self.block_work_stack.pop() else { break };
            bb = next;
        }
    }

    fn solve(&mut self) {
        while let Some(inst) = self.inst_work_stack.pop() {
            match self.func.dfg.insts[inst] {
                InstructionData::Branch { then_dst, else_dst, .. } => {
                    let bb = self.func.layout.inst_block(inst).unwrap();
                    let end = self.dom_tree.ipdom(bb);
                    self.taint_block(then_dst, end);
                    self.taint_block(else_dst, end);
                }
                InstructionData::Jump { destination } => {
                    for inst in self.func.layout.block_insts(destination) {
                        if self.func.dfg.insts[inst].is_phi() {
                            self.taint_inst(inst)
                        } else {
                            break;
                        }
                    }
                }
                _ => {
                    for use_ in self.func.dfg.inst_uses(inst) {
                        let user = self.func.dfg.use_to_user(use_);
                        self.taint_inst(user);
                    }
                }
            }
        }
    }
}
