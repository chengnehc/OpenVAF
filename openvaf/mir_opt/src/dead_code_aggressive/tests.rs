use bitset::{BitSet, SparseBitMatrix};
use mir::{ControlFlowGraph, DominatorTree, Value};

use expect_test::{expect, Expect};
use mir_reader::parse_function;

use crate::{aggressive_dead_code_elimination, simplify_cfg};

fn check(src: &str, expect: Expect, outputs: &BitSet<Value>) {
    let (mut func, _) = parse_function(src).unwrap();
    let mut cfg = ControlFlowGraph::with_function(&func);
    let dom_tree = DominatorTree::with_func_and_cfg::<true, true>(&func, &cfg);

    let mut control_dep = SparseBitMatrix::new_square(0);
    dom_tree.compute_postdom_frontiers(&cfg, &mut control_dep);

    aggressive_dead_code_elimination(
        &mut func,
        &mut cfg,
        &|val, _| outputs.contains(val),
        &control_dep,
    );
    simplify_cfg(&mut func, &mut cfg);

    expect.assert_eq(&func.to_debug_string());
}

#[test]
fn cmu() {
    let src = r#"
        function %foo() {
            v4 = iconst 0
            v5 = iconst 1
            v18 = iconst 100
        block8:
            v20 = optbarrier v5
            jmp block2

        block2:
            v17 = phi [v31, block3], [v4, block8]
            v19 = ilt v17, v18
            br v19, block3, block1

        block3:
            v31 = iadd v17, v5
            jmp block2

        block1:
        }
        "#;

    let expect = expect![[r#"
        function %foo() {
            v5 = iconst 1
        block8:
            v20 = optbarrier v5
        }
    "#]];

    let mut outputs = BitSet::new_empty(50);
    outputs.insert(20u32.into());

    check(src, expect, &outputs);
}
