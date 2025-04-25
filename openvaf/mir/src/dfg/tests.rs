use expect_test::expect;

use crate::instructions::{Opcode, PhiMap, PhiNode};
use crate::{Block, F_ZERO};

use super::*;

#[test]
fn make_inst() {
    let mut dfg = DataFlowGraph::new();
    let v1 = dfg.make_param(0u32.into());

    let data = InstructionData::Binary { opcode: Opcode::Fadd, args: [F_ZERO, v1] };
    let inst = dfg.make_inst(data);
    dfg.make_inst_results(inst);

    assert_eq!(inst.to_string(), "inst0");
    expect![[r#"
        "v17 = fadd v3, v16"
    "#]]
    .assert_debug_eq(&dfg.display_inst(inst).to_string());

    let v2 = dfg.first_result(inst);
    assert_eq!(dfg.inst_results(inst), &[v2]);
    assert_eq!(dfg.value_def(v2), ValueDef::Result(inst, 0));
    // v2 is attached (to an instruction as its result)
    assert!(dfg.value_attached(v2));
    // v2 is not used anywhere
    assert_eq!(dfg.uses(v2).count(), 0);

    let data = InstructionData::Binary { opcode: Opcode::Fadd, args: [v2, v2] };
    let inst = dfg.make_inst(data);
    dfg.make_inst_results(inst);

    let v3 = dfg.first_result(inst);
    assert_eq!(dfg.value_def(v3), ValueDef::Result(inst, 0));
    // `v3` is not used anywhere.
    assert_eq!(dfg.uses(v3).count(), 0);
    // `v2` is used twice by `inst` as its operands.
    assert_eq!(dfg.uses(v2).count(), 2);

    assert_eq!(dfg.uses(v2).rev().collect::<Vec<_>>(), dfg.operands(inst));

    // test that updating is a noop when nothing has changed
    dfg.zap_inst(inst);
    dfg.update_inst_uses(inst);
    assert_eq!(dfg.uses(v2).count(), 2);

    dfg.replace_uses(v2, F_ZERO);
    assert_eq!(dfg.instr_args(inst), &[F_ZERO, F_ZERO]);
    assert_eq!(dfg.uses(F_ZERO).count(), 3);
    assert!(dfg.value_dead(v2));
    assert_eq!(dfg.uses(v2).count(), 0);

    dfg.zap_inst(inst);
    assert_eq!(dfg.uses(v1).count(), 1);
    assert_eq!(dfg.uses(F_ZERO).count(), 1);

    dfg.instr_args_mut(inst).copy_from_slice(&[v1, v2]);
    dfg.update_inst_uses(inst);
    assert_eq!(dfg.uses(v2).count(), 1);
    assert_eq!(dfg.uses(v1).count(), 2);
    assert_eq!(dfg.uses(F_ZERO).count(), 1);
}

#[test]
fn phi() {
    let mut dfg = DataFlowGraph::new();
    let v3 = dfg.fconst(2f64.into());
    let v4 = dfg.fconst(4f64.into());
    let b0 = Block::from(0u32);
    let b1 = Block::from(1u32);

    let inst = dfg.make_inst(PhiNode { args: ValueList::new(), blocks: PhiMap::new() }.into());
    assert_eq!(
        dfg.insts[inst].unwrap_phi().edges(&dfg.insts.value_lists, &dfg.phi_forest).count(),
        0
    );

    dfg.insert_phi_edge(inst, b0, F_ZERO);
    dfg.insert_phi_edge(inst, b1, v3);
    assert_eq!(dfg.uses(F_ZERO).count(), 1);
    assert_eq!(dfg.uses(v3).count(), 1);
    assert_eq!(
        dfg.insts[inst]
            .unwrap_phi()
            .edges(&dfg.insts.value_lists, &dfg.phi_forest)
            .collect::<Vec<_>>(),
        &[(b0, F_ZERO), (b1, v3)]
    );

    dfg.insert_phi_edge(inst, b1, v4);
    assert_eq!(dfg.uses(F_ZERO).count(), 1);
    assert_eq!(dfg.uses(v4).count(), 1);
    assert_eq!(dfg.uses(v3).count(), 0);
    assert_eq!(
        dfg.insts[inst]
            .unwrap_phi()
            .edges(&dfg.insts.value_lists, &dfg.phi_forest)
            .collect::<Vec<_>>(),
        &[(b0, F_ZERO), (b1, v4)]
    );
    assert_eq!(
        dfg.insts[inst].unwrap_phi().edge_val_of(b0, &dfg.insts.value_lists, &dfg.phi_forest),
        Some(F_ZERO)
    );
    assert_eq!(dfg.try_remove_phi_edge(inst, b0), Some(F_ZERO));

    assert_eq!(dfg.uses(F_ZERO).count(), 0);
    assert_eq!(dfg.uses(v3).count(), 0);
    assert_eq!(dfg.uses(v4).count(), 1);
    assert_eq!(
        dfg.insts[inst]
            .unwrap_phi()
            .edges(&dfg.insts.value_lists, &dfg.phi_forest)
            .collect::<Vec<_>>(),
        &[(b1, v4)]
    );
}
