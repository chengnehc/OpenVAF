use super::InstBuilder;
use crate::cursor::{Cursor, FuncCursor};
use crate::{Function, ValueDef};

#[test]
fn reuse_results() {
    let mut func = Function::new();
    let block0 = func.layout.append_new_block();
    let arg0 = func.dfg.make_param(0u32.into());
    let mut pos = FuncCursor::new(&mut func).at_bottom(block0);

    let c0 = pos.func.dfg.iconst(17);
    let v0 = pos.ins().iadd(arg0, c0);
    let v1 = pos.ins().imul(v0, c0);
    let imul = pos.prev_inst().unwrap();

    // Detach `v1` from `imul`
    pos.func.dfg.clear_results(imul);
    // Create `iadd` reusing result `v1`
    pos.ins().with_result(v1).iadd(arg0, c0);
    // the cursor should now be positioned at `imul`
    assert_eq!(pos.current_inst(), Some(imul));
    // `iadd` should be the previous instruction
    let iadd = pos.prev_inst().unwrap();
    // `v1` should be the first result of `iadd`
    assert_eq!(pos.func.dfg.value_def(v1), ValueDef::Result(iadd, 0));
}
