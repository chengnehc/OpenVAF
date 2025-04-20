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
use crate::WITH_CONTRIBUTES;

fn run_test(src: &str) {
    let db = CompilationDB::new_from_vfs(src).unwrap();
    let module = crate::collect_modules(&db, false, &mut ConsoleSink::new(&db)).unwrap().remove(0);
    let mut literals = Rodeo::new();
    let mut context = Context::new(&db, &mut literals, &module);
    context.compute_outputs::<WITH_CONTRIBUTES>();
    context.compute_cfg();
    context.optimize(OptimizationStage::Initial);

    context.init_op_dependent_insts();

    let topo = Topology::new(&mut context);
    let mut dae = DaeSystem::new(&mut context, topo);
    context.compute_cfg();
    context.optimize(OptimizationStage::Final);
    dae.sparsify(&mut context);

    let test_dir = openvaf_test_data("dae");
    let name = module.module.name(&db);
    let dae = format!("{dae:#?}");
    expect_file![test_dir.join(format!("{name}_system.snap"))].assert_eq(&dae);
    // let _ = std::fs::write(test_dir.join(format!("{name}_system.snap")), &dae);

    assert!(context.func.validate());
    let func = format!("{:#?}", context.func);
    expect_file![test_dir.join(format!("{name}_mir.snap"))].assert_eq(&func)
    // let _ = std::fs::write(test_dir.join(format!("{name}_mir.snap")), &func);
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

/// A resistor written in standard KCL form.
#[test]
fn conductance() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module conductance(inout a, inout b);
            electrical a, b;
            parameter real G = 1.0;
            analog begin
                I(a, b) <+ V(a, b) * G;
            end
        endmodule
    "#};
    run_test(src);
}

/// A resistor written in non-KCL form. An extra branch flow unknown is required.
#[test]
fn resistance() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module resistance(inout a, inout b);
            electrical a, b;
            parameter real R = 1.0;
            analog begin
                V(a, b) <+ I(a, b) * R;
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn capacitance_ddt() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module capacitance_ddt(inout a, inout b);
            electrical a, b;
            parameter real C = 1.0;
            analog begin
                I(a, b) <+ ddt(V(a, b)) * C;
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn inductance_idt() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module inductance_idt(inout a, inout b);
            electrical a, b;
            parameter real L = 1.0;
            analog begin
                I(a, b) <+ idt(V(a, b)) / L;
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn inductance_ddt() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module inductance_ddt(inout a, inout b);
            electrical a, b;
            parameter real L = 1.0;
            analog begin
                V(a, b) <+ ddt(I(a, b)) * L;
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn current_src() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module current_src(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0;
            analog begin
                I(a, c) <+ foo;
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
fn vcvs() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module vcvs(inout pout, inout nout, inout pin, inout nin);
            electrical pout, nout, pin, nin;
            parameter real gain = 1.0;
            analog begin
                V(pout, nout) <+ V(pin, nin) * gain;
            end
        endmodule
    "#};
    run_test(src);
}

#[test]
fn vccs() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module vccs(inout pout, inout nout, inout pin, inout nin);
            electrical pout, nout, pin, nin;
            parameter real gain = 1.0;
            analog begin
                I(pout, nout) <+ V(pin, nin) * gain;
            end
        endmodule
    "#};
    run_test(src);
}

// #[test]
// fn ccvs() {
//     let src = indoc! {r#"
//         `include "disciplines.vams"
//         module ccvs(inout pout, inout nout, inout pin, inout nin);
//             electrical pout, nout, pin, nin;
//             parameter real gain = 1.0;
//             analog begin
//                 V(pout, nout) <+ I(pin, nin) * gain;
//             end
//         endmodule
//     "#};
//     run_test(src);
// }

// #[test]
// fn cccs() {
//     let src = indoc! {r#"
//         `include "disciplines.vams"
//         module cccs_va(Inp,Inm,Outp,Outm);
//             // ideal current controlled current source
//             inout Inp, Inm,Outp,Outm;
//             electrical Inp, Inm,Outp,Outm;

//             branch (Inp,Inm) br_in;
//             branch (Outp,Outm) br_out;

//             (*desc= "current amplification factor"*) parameter real G = 10 from [0:inf];
//             (*desc= "input Resistance", units = "Ohm" *) parameter real Rin = 1 from [-inf:inf];
//             (*desc= "output Resistance", units = "Ohm" *) parameter real Rout = 1e9 from [-inf:inf];

//             real Iin,Vout,Vin;

//             analog begin
//                 Iin = I(br_in);
//                 Vout = V(br_out);
//                 Vin = V(br_in);
//                 I(br_out) <+ Iin*G+Vout/Rout;
//                 I(br_in) <+ Vin/Rin;
//             end
//         endmodule
//     "#};
//     run_test(src);
// }

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
