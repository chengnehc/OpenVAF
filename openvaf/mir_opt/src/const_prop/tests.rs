use expect_test::{expect, Expect};
use mir::ControlFlowGraph;
use mir_reader::parse_function;

fn check(src: &str, data_flow_result: Expect) {
    let (mut func, _) = parse_function(src).unwrap();
    let cfg = ControlFlowGraph::with_function(&func);

    crate::sparse_conditional_constant_propagation(&mut func, &cfg);
    data_flow_result.assert_eq(&func.to_debug_string());
}

#[test]
pub fn const_phi() {
    let raw = r#"
        function %bar(v20) {
        block0:
            v21 = imul v4, v20
            v22 = ieq v4, v21
            br v22, block1, block2
        block1:
            v23 = iadd v5, v5
            jmp block3
        block2:
            v24 = isub v5, v5
            jmp block3
        block3:
            v25 = phi [v23, block1], [v24, block2]
            v26 = imul v25, v20
        }
    "#;

    let expect = expect![[r#"
        function %bar(v20) {
            // v2 = bconst true
            v27 = iconst 2
        block0:
            br v2, block1, block2

        block1:
            jmp block3

        block2:
            jmp block3

        block3:
            v26 = imul v27, v20
        }
    "#]];

    check(raw, expect)
}

#[test]
pub fn no_const_backward_edge() {
    let raw = r#"
        function %bar(v20) {
            v30 = iconst 20
        block0:
            jmp block1
        block1:
            v21 = phi [v4, block0], [v23, block2]
            v22 = ilt v21, v30
            br v22, block2, block3
        block2:
            v23 = iadd v21, v5
            jmp block1
        block3:
            v24 = imul v22, v20
        }
    "#;

    let expect = expect![[r#"
        function %bar(v20) {
            v4 = iconst 0
            v5 = iconst 1
            v30 = iconst 20
        block0:
            jmp block1

        block1:
            v21 = phi [v4, block0], [v23, block2]
            v22 = ilt v21, v30
            br v22, block2, block3

        block2:
            v23 = iadd v21, v5
            jmp block1

        block3:
            v24 = imul v22, v20
        }
    "#]];

    check(raw, expect)
}

#[test]
pub fn const_backward_edge() {
    let raw = r#"
        function %bar(v20) {
            v30 = iconst 20
        block0:
            jmp block1
        block1:
            v21 = phi [v30, block0], [v23, block2]
            v22 = ilt v21, v30
            br v22, block2, block3
        block2:
            v23 = iadd v21, v5
            jmp block1
        block3:
            v24 = imul v21, v20
        }
    "#;

    let expect = expect![[r#"
        function %bar(v20) {
            // v1 = bconst false
            v30 = iconst 20
        block0:
            jmp block1

        block1:
            br v1, block2, block3

        block2:
            jmp block1

        block3:
            v24 = imul v30, v20
        }
    "#]];

    check(raw, expect)
}

// The example is taken from: Todd C. Mowry, Lecture 14: SSA-Style Optimizations, CMU
#[test]
pub fn cmu() {
    let raw = r#"
        function %foo() {
            v4 = iconst 0
            v5 = iconst 1
            v18 = iconst 100
            v23 = iconst 20
            v33 = iconst 2
        block8:
            jmp block0
        block0:
            jmp block2
        block2:
            v22 = phi [v5, block0], [v36, block7]
            v17 = phi [v4, block0], [v35, block7]
            v19 = ilt v17, v18
            br v19, block3, block1
        block3:
            v24 = ilt v22, v23
            br v24, block5, block6
        block5:
            v31 = iadd v17, v5
            jmp block7
        block6:
            v34 = iadd v17, v33
            jmp block7
        block7:
            v36 = phi [v5, block5], [v17, block6]
            v35 = phi [v31, block5], [v34, block6]
            jmp block2
        block1:
    }
    "#;

    let expect = expect![[r#"
        function %foo() {
            // v2 = bconst true
            v4 = iconst 0
            v5 = iconst 1
            v18 = iconst 100
        block8:
            jmp block0
        
        block0:
            jmp block2
        
        block2:
            v17 = phi [v4, block0], [v35, block7]
            v19 = ilt v17, v18
            br v19, block3, block1
        
        block3:
            br v2, block5, block6
        
        block5:
            v31 = iadd v17, v5
            jmp block7
        
        block6:
            jmp block7
        
        block7:
            v35 = phi [v31, block5], [v34, block6]
            jmp block2
        
        block1:
        }
    "#]];

    check(raw, expect)

    // # Note
    // Terminators are not eliminated directly by SCCP, so are the unreachable
    // blocks. This is done later by `simplify_cfg()`, after which the MIR becomes:
    //
    // function %foo() {
    //     v4 = iconst 0
    //     v5 = iconst 1
    //     v18 = iconst 100
    // block8:
    //     jmp block2
    // block2:
    //     v17 = phi [v31, block3], [v4, block8]
    //     v19 = ilt v17, v18
    //     br v19, block3, block1
    // block3:
    //     v31 = iadd v17, v5
    //     jmp block2
    // block1:
    // }
    //
    // which is the same as what the diagram on the slide presents
}
