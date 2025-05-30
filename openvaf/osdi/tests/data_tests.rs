use camino::Utf8Path;
use paths::AbsPathBuf;
use std::path::Path;

use hir::diagnostics::ConsoleSink;
use hir::CompilationDB;
use llvm::OptLevel;
use mir_llvm::LLVMBackend;
use target::spec::Target;

use mini_harness::{harness, Result};
use stdx::{ignore_slow_tests, project_root};

fn test_compile(root_file: &Path) -> Result {
    let root_file = AbsPathBuf::assert(root_file.canonicalize()?);
    let db = CompilationDB::new_from_fs(root_file, &[], &[], &[])?;
    let modules = hir::collect_modules(&db, false, &mut ConsoleSink::new(&db)).unwrap();
    let target = Target::host_target().unwrap();
    let back = LLVMBackend::new(&[], &target, "native", &[]);

    const EMIT: bool = !stdx::IS_CI;
    let osdi_dir = project_root().join("openvaf").join("osdi");
    let dst = osdi_dir.join("foo");
    let dst = Utf8Path::from_path(dst.as_path()).unwrap();
    let objs = osdi::compile::<EMIT>(&db, &modules, dst, &back, OptLevel::None);
    if EMIT {
        for obj in objs {
            std::fs::remove_file(obj)?;
        }
    }

    Ok(())
}

fn integration(dir: &Path) -> Result {
    let name = dir.file_name().unwrap().to_str().unwrap().to_lowercase();
    let main_file = dir.join(name).with_extension("va");
    test_compile(&main_file)?;

    Ok(())
}

harness! {
    Test::from_dir("integration", &integration, &ignore_slow_tests, &project_root().join("integration_tests"))
}
