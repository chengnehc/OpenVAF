use std::fs;
use stdx::{integration_test_dir, openvaf_test_data};

use expect_test::expect_file;
use hir::{CompilationDB, ScopeDef};
use lasso::Rodeo;
use mir::Function;

use crate::HirInterner;

fn run_test(src: &str) {
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

    let test_dir = openvaf_test_data("param");
    let name = module.name(&db);
    let actual = model_param_setup.to_debug_string();

    // let _ = fs::write(test_dir.join(format!("{name}_model_param_setup.mir")), &actual);
    expect_file![test_dir.join(format!("{name}_model_param_setup.mir"))].assert_eq(&actual);
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
