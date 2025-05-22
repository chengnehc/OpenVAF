use super::*;

use hir::ScopeDef;

use expect_test::expect_file;
use indoc::indoc;
use stdx::{integration_test_dir, openvaf_test_data};

fn test_setup_model(src: &str) {
    let db = CompilationDB::new_from_vfs(src).unwrap();
    let cu = db.compilation_unit();
    let mut literals = Rodeo::new();

    let mut model_param_setup = Function::default();
    let mut model_param_intern = HirInterner::default();

    let module = cu.modules(&db)[0];

    let model_params: Vec<_> = module
        .rec_declarations(&db)
        .filter_map(
            |(_, def)| if let ScopeDef::Parameter(param) = def { Some(param) } else { None },
        )
        .collect();

    model_param_intern.insert_param_init(
        &db,
        &mut model_param_setup,
        &mut literals,
        false,
        true,
        &model_params,
    );

    assert!(model_param_setup.validate());

    let test_dir = openvaf_test_data("param");
    let name = module.name(&db);
    let actual = model_param_setup.to_debug_string();

    // let _ = fs::write(test_dir.join(format!("{name}.mir")), &actual);
    expect_file![test_dir.join(format!("{name}.mir"))].assert_eq(&actual);
}

#[test]
fn diode() {
    let src = std::fs::read_to_string(integration_test_dir("DIODE").join("diode.va")).unwrap();
    test_setup_model(&src);
}

#[test]
fn resistor() {
    let src =
        std::fs::read_to_string(integration_test_dir("RESISTOR").join("resistor.va")).unwrap();
    test_setup_model(&src);
}

#[test]
fn include_range() {
    let src = indoc! {r#"
        module include_range;
            parameter integer n = 1 from (0:1];
            parameter real m = 0 from [0:1);
        endmodule
    "#};

    test_setup_model(src);
}

#[test]
fn infinity_bound() {
    let src = indoc! {r#"
        module infinity_bound;
            parameter integer i = 1 from [0:inf);
            parameter real f = -1.0 from (-inf:0];
        endmodule
    "#};

    test_setup_model(src);
}

#[test]
fn include_range_union() {
    let src = indoc! {r#"
        module include_range_union;
            parameter integer n = 1 from (0:10) from (20:30);
        endmodule
    "#};

    test_setup_model(src);
}

#[test]
fn exclude_singularity() {
    let one = indoc! {r#"
        module exclude_singularity_one;
            parameter real zeta = 0.5 from (-1:1) exclude 0;
        endmodule
    "#};

    let many = indoc! {r#"
        module exclude_singularity_many;
            parameter real m = 0 from (-inf:inf) exclude -1 exclude 1;
        endmodule
    "#};

    test_setup_model(one);
    test_setup_model(many);
}

/// Excluding a range is the same as including the complement range of it.
/// Compact models seldom use this.
#[test]
fn exclude_range() {
    let src = indoc! {r#"
        module exclude_range;
            parameter integer pos = 1 exclude (-inf:0]
            // should be the same as 'from (0:inf)'
        endmodule
    "#};

    test_setup_model(src);
}

/// Test parameter whose default value is correlated with another parameter's
/// run-time value. This snap is taken from BSIM-CMG.
#[test]
fn correlated_parameter() {
    let src = indoc! {r#"
        `define BPRcz(nam, def, uni, des) (* units = uni, type = "instance", desc = des *) parameter real nam = def from[0.0 : inf);
        module correlated_parameter;
            `BPRcz(COVS, 0.0, "F/m", "Constant gate-to-source overlap capacitance (CGEOMOD = 1)")
            `BPRcz(COVD, COVS, "F/m", "Constant gate-to-drain overlap capacitance (CGEOMOD = 1)")
        endmodule
    "#};

    test_setup_model(src);
}
