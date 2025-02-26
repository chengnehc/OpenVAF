use std::path::Path;

use basedb::AbsPathBuf;
use hir::CompilationDB;

use expect_test::expect_file;
use mini_harness::{harness, Result};
use stdx::{ignore_dev_tests, ignore_never, is_va_file, openvaf_test_data, project_root};

fn integration(dir: &Path) -> Result {
    let name = dir.file_name().unwrap().to_str().unwrap().to_lowercase();
    let root_file = dir.join(name).with_extension("va").canonicalize().unwrap();
    let db = CompilationDB::new_from_fs(AbsPathBuf::assert(root_file), &[], &[], &[]).unwrap();
    let actual = db.compilation_unit().test_diagnostics(&db);

    //std::fs::write(dir.join("frontend.log"), actual)?;
    expect_file![dir.join("frontend.log")].assert_eq(&actual);

    Ok(())
}

fn ui(file: &Path) -> Result {
    let db =
        CompilationDB::new_from_fs(AbsPathBuf::assert(file.canonicalize().unwrap()), &[], &[], &[])
            .unwrap();
    let actual = db.compilation_unit().test_diagnostics(&db);

    expect_file![file.with_extension("log")].assert_eq(&actual);

    Ok(())
}

harness! {
    Test::from_dir_filtered("integration", &integration, &Path::is_dir, &ignore_dev_tests, &project_root().join("integration_tests")),
    Test::from_dir_filtered("ui", &ui, &is_va_file, &ignore_never, &openvaf_test_data("ui"))
}
