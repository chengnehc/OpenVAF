use std::{iter, path::Path};

use hir::CompilationDB;
use hir_lower::{MirBuilder, PlaceKind};
use mir::{Function, Rodeo};

use basedb::AbsPathBuf;
use expect_test::expect_file;
use mini_harness::{harness, Result};
use stdx::{ignore_dev_tests, ignore_never, is_va_file, openvaf_test_data, project_root};

fn lower_to_mir(db: &CompilationDB, is_output: &impl Fn(PlaceKind) -> bool) -> Vec<Function> {
    let cu = db.compilation_unit();

    // ensure the HIR contains no errors first
    let fatal = cu.test_diagnostics(db).0;
    assert!(!fatal, "HIR contains fatal diagnostics");

    cu.modules(db)
        .iter()
        .map(|&module| {
            let mut required_vars = iter::empty();
            let mut literals = Rodeo::new();
            let (mir, _) =
                MirBuilder::new(db, module, is_output, &mut required_vars).build(&mut literals);
            mir
        })
        .collect()
}

fn integration(dir: &Path) -> Result {
    let name = dir.file_name().unwrap().to_str().unwrap().to_lowercase();
    let main_file = dir.join(format!("{name}.va")).canonicalize()?;
    let db = CompilationDB::new_from_fs(AbsPathBuf::assert(main_file), &[], &[], &[])?;
    let is_output =
        |kind| matches!(kind, PlaceKind::Contribute { .. } | PlaceKind::ImplicitResidual { .. });

    lower_to_mir(&db, &is_output);

    Ok(())
}

fn mir(file: &Path) -> Result {
    let db = CompilationDB::new_from_fs(AbsPathBuf::assert(file.canonicalize()?), &[], &[], &[])?;
    // for testing, all variables are treated as output
    let is_output = |kind| {
        matches!(
            kind,
            PlaceKind::Contribute { .. } | PlaceKind::ImplicitResidual { .. } | PlaceKind::Var(_)
        )
    };

    let actual = lower_to_mir(&db, &is_output)[0].to_debug_string();
    // std::fs::write(file.with_extension("mir"), actual)?;
    expect_file![file.with_extension("mir")].assert_eq(&actual);

    Ok(())
}

harness! {
    Test::from_dir_filtered("integration", &integration, &Path::is_dir, &ignore_dev_tests, &project_root().join("integration_tests")),
    Test::from_dir_filtered("mir", &mir, &is_va_file, &ignore_never, &openvaf_test_data("mir"))
}
