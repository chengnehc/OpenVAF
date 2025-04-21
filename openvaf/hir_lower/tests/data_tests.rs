use std::path::Path;

use basedb::AbsPathBuf;
use hir::CompilationDB;
use hir_lower::{MirBuilder, PlaceKind};
use lasso::Rodeo;
use mir::Function;
use mir_build::FunctionBuilderContext;

use expect_test::expect_file;
use mini_harness::{harness, Result};
use stdx::{ignore_dev_tests, ignore_never, is_va_file, openvaf_test_data, project_root};

fn lower_to_mir(db: &CompilationDB, is_output: &impl Fn(PlaceKind) -> bool) -> Vec<Function> {
    let unit = db.compilation_unit();
    // ensure that the HIR contains no errors first
    assert_eq!(unit.test_diagnostics(db), "");
    // a builder context that remains undropped across different lowering pass
    let mut func_ctxt = FunctionBuilderContext::default();

    unit.modules(db)
        .iter()
        .map(|&module| {
            let mut required_vars = [].into_iter();
            let mut literals = Rodeo::new();
            let (mir, _) = MirBuilder::new(db, module, is_output, &mut required_vars)
                .with_func_builder_context(&mut func_ctxt)
                .build(&mut literals);
            mir
        })
        .collect()
}

fn integration(dir: &Path) -> Result {
    let name = dir.file_name().unwrap().to_str().unwrap().to_lowercase();
    let main_file = dir.join(format!("{name}.va")).canonicalize().unwrap();
    let db =
        CompilationDB::new_from_fs(AbsPathBuf::assert(main_file.clone()), &[], &[], &[]).unwrap();
    let is_output =
        |kind| matches!(kind, PlaceKind::Contribute { .. } | PlaceKind::ImplicitResidual { .. });

    lower_to_mir(&db, &is_output);
    // let mirs = lower_to_mir(&db, &is_output);
    // let actual = mirs[0].to_debug_string();
    // std::fs::write(main_file.with_extension("mir"), actual)?;

    Ok(())
}

fn mir(file: &Path) -> Result {
    let db =
        CompilationDB::new_from_fs(AbsPathBuf::assert(file.canonicalize().unwrap()), &[], &[], &[])
            .unwrap();
    let is_output = |kind| {
        matches!(
            kind,
            PlaceKind::Contribute { .. } | PlaceKind::ImplicitResidual { .. } | PlaceKind::Var(_)
        )
    };

    let mirs = lower_to_mir(&db, &is_output);
    let actual = mirs[0].to_debug_string();

    // std::fs::write(file.with_extension("mir"), actual)?;
    expect_file![file.with_extension("mir")].assert_eq(&actual);

    Ok(())
}

harness! {
    Test::from_dir_filtered("integration", &integration, &Path::is_dir, &ignore_dev_tests, &project_root().join("integration_tests")),
    Test::from_dir_filtered("mir", &mir, &is_va_file, &ignore_never, &openvaf_test_data("mir"))
}
