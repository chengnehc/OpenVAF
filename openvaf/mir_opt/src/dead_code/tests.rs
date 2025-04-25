use bitset::BitSet;
use mir::Value;

use expect_test::{expect, Expect};
use mir_reader::parse_function;

use super::standard_dead_code_elimination;

fn check(src: &str, expect: Expect, outputs: &BitSet<Value>) {
    let (mut func, _) = parse_function(src).unwrap();

    standard_dead_code_elimination(&mut func, outputs);
    expect.assert_eq(&func.to_debug_string());
}

/// Standard dce cannot simplify this
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
    "#]];

    let mut outputs = BitSet::new_empty(50);
    outputs.insert(20u32.into());

    check(src, expect, &outputs);
}
