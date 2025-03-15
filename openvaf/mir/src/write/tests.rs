use expect_test::expect;

use crate::builder::InstBuilder;
use crate::cursor::{Cursor, CursorPosition, FuncCursor};
use crate::Function;

#[test]
fn basic() {
    let mut func = Function::with_name("foo".to_owned());

    let v1 = func.dfg.make_param(0u32.into());
    let v2 = func.dfg.make_param(1u32.into());
    let v3 = func.dfg.iconst(3);

    let block = func.layout.append_new_block();
    let mut cursor = FuncCursor::new(&mut func);
    cursor.set_position(CursorPosition::After(block));

    let v4 = cursor.ins().iadd(v1, v2);
    cursor.ins().isub(v4, v3);

    let expected = expect![[r#"
        function %foo(v16, v17) {
            v18 = iconst 3
        block0:
            v19 = iadd v16, v17
            v20 = isub v19, v18
        }
    "#]];
    expected.assert_eq(&func.to_debug_string());
}
