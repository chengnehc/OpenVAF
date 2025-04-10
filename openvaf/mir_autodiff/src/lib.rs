use ahash::AHashMap;
use mir::{DataFlowGraph, DominatorTree, Function, Inst, InstructionData, Opcode, Value};
use mir::{KnownDerivatives, Unknown};

mod builder;
mod intern;
mod live_derivatives;
mod postorder;
mod subgraph;

use intern::{Derivative, DerivativeIntern};

pub use builder::build_derivatives;
pub use live_derivatives::LiveDerivatives;

/// The main entrance routine for doing auto-differentiation.
///
/// Return a map from derivative operands to its result value.
pub fn auto_diff(
    mut func: impl AsMut<Function>,
    dom_tree: &DominatorTree,
    known_derivatives: &KnownDerivatives, // ddx calls explicitly specified by user code
    extra_derivatives: &[(Value, Unknown)], // Jacobians
) -> AHashMap<(Value, Unknown), Value> {
    let func = func.as_mut();
    let mut intern = DerivativeIntern::new(known_derivatives);
    let live_derivatives = LiveDerivatives::build(func, &mut intern, extra_derivatives, dom_tree);

    build_derivatives(func, &mut intern, &live_derivatives, dom_tree.cfg_postorder())
}

fn is_zero_call(dfg: &DataFlowGraph, inst: Inst, intern: &DerivativeIntern) -> bool {
    if let InstructionData::Call { func_ref, .. } = dfg.insts[inst] {
        !intern.ddx_calls.contains_key(&func_ref)
    } else {
        false
    }
}

fn zero_derivative(dfg: &DataFlowGraph, inst: Inst) -> bool {
    let opcode = dfg.insts[inst].opcode();
    matches!(
        opcode,
        Opcode::Ineg
            | Opcode::Iadd
            | Opcode::Isub
            | Opcode::Imul
            | Opcode::Idiv
            | Opcode::Ishl
            | Opcode::Ishr
            | Opcode::IFcast
            | Opcode::BIcast
            | Opcode::IBcast
            | Opcode::FBcast
            | Opcode::BFcast
            | Opcode::FIcast
            | Opcode::Irem
            | Opcode::Inot
            | Opcode::Ixor
            | Opcode::Iand
            | Opcode::Ior
            | Opcode::Clog2
            | Opcode::Frem
            | Opcode::Floor
            | Opcode::Ceil
            | Opcode::Bnot
            | Opcode::Ilt
            | Opcode::Igt
            | Opcode::Flt
            | Opcode::Fgt
            | Opcode::Ile
            | Opcode::Ige
            | Opcode::Fle
            | Opcode::Fge
            | Opcode::Ieq
            | Opcode::Feq
            | Opcode::Seq
            | Opcode::Beq
            | Opcode::Ine
            | Opcode::Fne
            | Opcode::Sne
            | Opcode::Bne
            | Opcode::Br
            | Opcode::Jmp
    )
}

#[derive(Debug, Clone)]
struct ChainRule {
    pub inner_derivative: (Value, Derivative),
    pub outer_derivative: Derivative,
    pub dst_derivative: Derivative,
    pub val: Value,
}
