use std::fs;

use hir::diagnostics::ConsoleSink;
use hir::CompilationDB;
use lasso::Rodeo;

use expect_test::expect_file;
use indoc::indoc;
use stdx::{integration_test_dir, openvaf_test_data};

use crate::context::{Context, OptimizationStage};
use crate::dae::DaeSystem;
use crate::topology::Topology;

fn run_test(src: &str) {
    let db = CompilationDB::new_from_vfs(src).unwrap();
    let module = crate::collect_modules(&db, false, &mut ConsoleSink::new(&db)).unwrap().remove(0);
    let mut literals = Rodeo::new();
    let mut context = Context::new(&db, &mut literals, &module);
    context.compute_outputs(true);
    context.compute_cfg();
    context.optimize(OptimizationStage::Initial);

    let topo = Topology::new(&mut context);
    let mut dae = DaeSystem::new(&mut context, topo);
    context.compute_cfg();
    context.optimize(OptimizationStage::Final);
    dae.sparsify(&mut context);

    let test_dir = openvaf_test_data("dae");
    let name = module.module.name(&db);
    let dae = format!("{dae:#?}");
    expect_file![test_dir.join(format!("{name}_system.snap"))].assert_eq(&dae);

    assert!(context.func.validate());
    let func = format!("{:#?}", context.func);
    expect_file![test_dir.join(format!("{name}_mir.snap"))].assert_eq(&func)
}

#[test]
fn diode() {
    let src = fs::read_to_string(integration_test_dir("DIODE").join("diode.va")).unwrap();
    run_test(&src);
}

#[test]
fn resistor() {
    let src = fs::read_to_string(integration_test_dir("RESISTOR").join("resistor.va")).unwrap();
    run_test(&src);
}

#[test]
fn lim_rhs() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module lim_rhs(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            analog begin
                I(a, c) <+ foo*exp($limit(V(a,c), "testlim"));
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn lim_rhs_react() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module lim_rhs_react(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            analog begin
                I(a, c) <+ ddt(foo*$limit(V(a,c), "testlim"));
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn lim_rhs_sign() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module lim_rhs_sign(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            real Vac;
            analog begin
                if (foo < 0) 
                    Vac = $limit(V(c, a), "testlim");
                else
                    Vac = $limit(V(a, c), "testlim");

                I(a, c) <+ foo*exp(Vac);
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn voltage_src() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module voltage_src(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0;
            analog begin
                V(a, c) <+ foo;
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn const_switch_branch() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module const_switch_branch(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0;
            analog begin
                if (foo < 0 )
                    V(a, c) <+ foo;
                else
                    I(a, c) <+ foo;
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn dyn_switch_branch() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module dyn_switch_branch(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0;
            analog begin
                if (V(a, c) < 0) 
                    V(a, c) <+ foo * V(a, c);
                else
                    I(a, c) <+ foo * V(a, c);
            end
        endmodule
    "#};
    run_test(src);
}
