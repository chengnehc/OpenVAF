use hir::diagnostics::ConsoleSink;
use hir::CompilationDB;
use lasso::Rodeo;
use mir::Function;

use expect_test::expect_file;
use indoc::indoc;
use stdx::openvaf_test_data;

use super::Topology;
use crate::context::{Context, OptimizationStage};
use crate::WITH_CONTRIBUTES;

fn compile(src: &str) -> (Function, Topology, String) {
    let db = CompilationDB::new_from_vfs(src).unwrap();
    let module = crate::collect_modules(&db, false, &mut ConsoleSink::new(&db)).unwrap().remove(0);
    let mut literals = Rodeo::new();
    let mut context = Context::new(&db, &mut literals, &module);
    context.compute_outputs::<WITH_CONTRIBUTES>();
    context.compute_cfg();
    context.optimize(OptimizationStage::Initial);
    let topology = Topology::new(&mut context);
    assert!(context.func.validate());

    (context.func, topology, module.module.name(&db))
}

fn assert(src: &str) {
    let (func, topology, name) = compile(src);
    println!("{func:?}");
    let test_dir = openvaf_test_data("topo");
    let func = format!("{func:#?}");
    // let _ = std::fs::write(test_dir.join(format!("{name}_mir.snap")), &func);
    expect_file![test_dir.join(format!("{name}_mir.snap"))].assert_eq(&func);
    let topology = format!("{topology:#?}");
    // let _ = std::fs::write(test_dir.join(format!("{name}_topo.snap")), &topology);
    expect_file![test_dir.join(format!("{name}_topo.snap"))].assert_eq(&topology);
}

#[test]
fn linear_analog_operators() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module linear_analog_operators(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            analog begin
                I(a, c) <+ V(a) + foo*ddt(V(a));
                I(a, c) <+ bar + foo*white_noise(2*bar, "bar");
            end
        endmodule
    "#};

    assert(src);
}

/// This testcase ensures three forms of ddt() are correctly handled:
/// 1. `Q = f(V(b_cap)); I(b_cap) <+ ddt(Q);` (based directly on charge)
/// 2. `C = f(V(b_cap)); I(b_cap) <+ C*ddt(V(b_cap));` (based directly on capacitance)
/// 3. `C = f(V(b_cap)); I(b_cap) <+ ddt(C*V(b_cap));` (by computing charge as Q = C*V)
///
/// - Form 1 is linear. It is recommended and should be used by compact models.
/// - Form 2 is not linear and not recommended, as it:
///     1. does not align with KCL nodal formulation and requires an implicit unknown/equation
///     2. makes it more difficult to guarantee that a model is charge conserving
/// - Although form 3 is linear, its result is incorrect when `C` is non-linear (op-dependent).
///
/// See Also: section IV of (1) and slide branch-ddt of (2)
///
/// 1. C. C. McAndrew et al., “Best Practices for Compact Modeling in Verilog-A,”
/// IEEE Journal of the Electron Devices Society, vol. 3, no. 5, pp. 383–396, Sep. 2015
/// 2. M. Mierzwinski, P. O’Halloran, and B. Troyanovsky, “Developing and releasing compact models using Verilog-A,”
/// in MOS-AK Workshop, San Francisco, CA, USA, Dec. 2008
#[test]
fn ddt() {
    let form_1 = indoc! {r#"
        `include "disciplines.vams"
        module ddt_form_1(inout a, inout c);
            electrical a, c;
            branch (a, c) cap;
            parameter real foo=1.0;
            real Q = 0.0;
            analog begin
                Q = foo * V(cap) * V(cap);
                I(cap) <+ ddt(Q);
            end
        endmodule
    "#};

    let form_2 = indoc! {r#"
        `include "disciplines.vams"
        module ddt_form_2(inout a, inout c);
            electrical a, c;
            branch (a, c) cap;
            parameter real foo=1.0;
            real C = 0.0;
            analog begin
                C = foo * V(cap);
                I(cap) <+ C * ddt(V(cap));
            end
        endmodule
    "#};

    let form_3 = indoc! {r#"
        `include "disciplines.vams"
        module ddt_form_3(inout a, inout c);
            electrical a, c;
            branch (a, c) cap;
            parameter real foo=1.0;
            real C = 0.0;
            analog begin
                C = foo * V(cap);
                I(cap) <+ ddt(C * V(cap));
            end
        endmodule
    "#};

    assert(form_1);
    assert(form_2);
    assert(form_3);
}

/// This testcase ensures that conditional time derivatives are evaluated properly.
///
/// In general, conditions are a bit tricky to handle. If ddt(x) is used in an
/// operating point dependent condition, we need to create a dedicated internal
/// node for it, so as to ensure the internal state gets updated correctly
/// (otherwise there would be discontinuities). This is fairly niche tough.
///
/// It's important to ensure that the more common case, i.e, using ddt(x) in
/// operating point independent conditions, also works.
#[test]
fn conditional_ddt() {
    cov_mark::check!(conditional_phi);
    let op_dependent = indoc! {r#"
        `include "disciplines.vams"
        module conditional_ddt_op_dependent(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            real tmp;
            analog begin
                I(a, c) <+ V(a);
                tmp = ddt(V(a));
                if (V(a) < 0) begin
                    if (foo < 0) begin
                        if (bar < 0) begin
                            I(a, c)  <+ tmp;
                        end
                    end
                end
            end
        endmodule
    "#};
    let op_independent = indoc! {r#"
        `include "disciplines.vams"
        module conditional_ddt_op_independent(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            analog begin
                if (foo < 0) begin
                    if (bar < 0) begin
                        I(a, c)  <+ ddt(V(c));
                    end
                end
            end
        endmodule
    "#};

    assert(op_dependent);
    assert(op_independent);
}

/// This testcase makes sure that the implicit equation caused by
/// un-linearize-able analog operators within can be collapsed under
/// specific parameter set, if the analog operator is defined within
/// conditionals.
#[test]
fn collapsible_implicit() {
    cov_mark::check!(collapsible_implicit);
    let src = indoc! {r#"
        `include "disciplines.vams"
        module collapsible_implicit(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0;
            analog begin
                if (foo < 0) begin
                    I(a, c)  <+ V(a) * ddt(V(c));
                end
            end
        endmodule
    "#};

    assert(src);
}

#[test]
fn unused_noise() {
    cov_mark::check!(dead_noise);
    let src = indoc! {r#"
        `include "disciplines.vams"
        module unused_noise(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            real tmp;
            analog begin
                tmp = V(a) + white_noise(2);
                if (tmp < 0) begin
                    I(a, c)  <+ V(a);
                end
            end
        endmodule
    "#};

    assert(src);
}

/// Noise doesn't really create its own equation (its just a small
/// signal source) so it can be used in conditions directly and can
/// stay linear even in conditional code.
#[test]
fn conditional_noise() {
    cov_mark::check!(linear_operator);
    let src = indoc! {r#"
        `include "disciplines.vams"
        module conditional_noise(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            analog begin
                I(a, c) <+ V(a);
                if (V(a) < 0) begin
                    I(a, c)  <+ white_noise(foo*bar);
                end
            end
        endmodule
    "#};

    assert(src);
}

/// This test covers two scenario:
/// * a noise source that is used multiple times is correctly transformed to a noise source.
/// * the contributions (to external nodes in this case) are correctly transformed to small
///   signal contributions
#[test]
fn correlated_noise() {
    cov_mark::check!(prune_small_signal);
    cov_mark::check!(port_not_small_signal);
    let src = indoc! {r#"
        `include "disciplines.vams"
        module correlated_noise(inout a, inout c);
            electrical a, c;
            parameter real foo=1.0, bar=2.0;
            real correlated_noise;
            analog begin
                correlated_noise = white_noise(foo);
                I(a) <+ V(a) + white_noise(foo) + bar * correlated_noise;
                I(c) <+ correlated_noise;
            end
        endmodule
    "#};

    assert(src);
}

#[test]
fn manual_correlated_noise() {
    cov_mark::check!(node_is_small_signal);
    let src = indoc! {r#"
        `include "disciplines.vams"
        module manual_correlated_noise(inout a, inout c);
            electrical noise, a, c;
            parameter real foo=1.0, bar=2.0;
            real correlated_noise;
            analog begin
                I(noise) <+ V(noise) - white_noise(foo);
                I(a, c) <+ V(a,c) / foo + 2*V(noise);
            end
        endmodule
    "#};

    assert(src);
}

#[test]
fn psp103() {
    let src = indoc! {r#"
        `include "disciplines.vams"
        module psp103(inout GP, inout SI, inout DI);
            electrical NOI, GP, SI, DI;
            branch (NOI) NOII;
            branch (NOI) NOIR;
            branch (NOI) NOIC;
            parameter real nt=0.0, mig=0.0, CGeff=0.0, MULT_i=0.0, migid=0.0, sqid=0.0, c_igid=0.0, sigVds=0.0;
            analog begin
                // subcircuit
                I(NOII) <+ white_noise((nt / mig));
                I(NOIR) <+ V(NOIR) / mig;
                I(NOIC) <+ ddt(CGeff * V(NOIC));
                // noise sources ids, igs, and igd
                I(GP,SI) <+ -ddt(sqrt(MULT_i) * 0.5 * CGeff * V(NOIC));
                I(GP,DI) <+ -ddt(sqrt(MULT_i) * 0.5 * CGeff * V(NOIC));
                I(DI,SI) <+ sigVds * sqrt(MULT_i) * migid * I(NOII);
                I(DI,SI) <+ white_noise(MULT_i * sqid * sqid * (1.0 - c_igid * c_igid));
            end
        endmodule
    "#};

    assert(src);
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
    assert(src);
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
    assert(src);
}

#[test]
fn switch_branch() {
    let constant = indoc! {r#"
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
    let dynamic = indoc! {r#"
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

    assert(constant);
    assert(dynamic);
}

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
    assert(src);
}

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
    assert(src);
}

#[test]
fn capacitance() {
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
    assert(src);
}

#[test]
fn inductance() {
    let idt = indoc! {r#"
        `include "disciplines.vams"
        module inductance_idt(inout a, inout b);
            electrical a, b;
            parameter real L = 1.0;
            analog begin
                I(a, b) <+ idt(V(a, b)) / L;
            end
        endmodule
    "#};

    let ddt = indoc! {r#"
        `include "disciplines.vams"
        module inductance_ddt(inout a, inout b);
            electrical a, b;
            parameter real L = 1.0;
            analog begin
                V(a, b) <+ ddt(I(a, b)) * L;
            end
        endmodule
    "#};

    assert(idt);
    assert(ddt);
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
    assert(src);
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
    assert(src);
}
