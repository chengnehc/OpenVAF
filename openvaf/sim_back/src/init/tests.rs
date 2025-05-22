use std::fs;

use hir::diagnostics::ConsoleSink;
use hir::CompilationDB;
use lasso::Rodeo;

use expect_test::expect_file;
use indoc::indoc;
use stdx::{integration_test_dir, openvaf_test_data};

use super::Initialization;
use crate::context::{Context, OptimizationStage};
use crate::dae::DaeSystem;
use crate::topology::Topology;
use crate::WITH_CONTRIBUTES;

fn run_test(src: &str) {
    let db = CompilationDB::new_from_vfs(src).unwrap();
    let module = hir::collect_modules(&db, false, &mut ConsoleSink::new(&db)).unwrap().remove(0);
    let mut literals = Rodeo::new();
    let mut ctx = Context::new(&db, &mut literals, &module);

    ctx.compute_outputs::<WITH_CONTRIBUTES>();
    ctx.compute_cfg();
    ctx.optimize(OptimizationStage::Initial);
    ctx.init_op_dependent_insts();

    let topology = Topology::new(&mut ctx);
    let mut dae_system = DaeSystem::new(&mut ctx, topology);

    ctx.compute_cfg();
    let gvn = ctx.optimize(OptimizationStage::PostDerivative);
    dae_system.sparsify(&mut ctx);
    ctx.refresh_op_dependent_insts();

    let init = Initialization::new(&mut ctx, &gvn);

    let name = module.module.name(&db);
    let test_dir = openvaf_test_data("init");

    assert!(ctx.func.validate());
    assert!(init.func.validate());

    let system = format!("{:#?}\n{:#?}", init.cached_vals, init.cache_slots);
    expect_file![test_dir.join(format!("{name}_system.snap"))].assert_eq(&system);

    let func = format!("{:#?}", init.func);
    expect_file![test_dir.join(format!("{name}_init_mir.snap"))].assert_eq(&func);

    let func = format!("{:#?}", &ctx.func);
    expect_file![test_dir.join(format!("{name}_eval_mir.snap"))].assert_eq(&func);
}

#[test]
fn diode() {
    let src = fs::read_to_string(integration_test_dir("DIODE").join("diode.va")).unwrap();
    run_test(&src);
}

#[test]
fn resistor() {
    cov_mark::check!(cache_output);
    cov_mark::check!(op_independent_output);
    let src = fs::read_to_string(integration_test_dir("RESISTOR").join("resistor.va")).unwrap();
    run_test(&src);
}

#[test]
fn op_dependent_collapse_hint() {
    cov_mark::check!(ignore_if_op_dependent);
    let src = indoc! {r#"
        `include "disciplines.vams"
        module op_dependent_collapse_hint(inout a, inout c);
            electrical a, c, d;
            parameter real foo=1.0;
            analog begin
                if (foo == 0) 
                    V(d) <+ 0.0;
                if (V(a, c) < 0) 
                    V(d) <+ 0.0;
                else
                    V(d) <+ foo;
                I(a, c) <+ V(d);
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn analysis() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module analysis_dependent(inout a, inout c);
            electrical a, c;
            parameter real R=1.0;
            analog begin
                if (analysis("tran"))
                    I(a, c) <+ R * V(a,c);
            end
        endmodule
    "#};
    run_test(src);
}
