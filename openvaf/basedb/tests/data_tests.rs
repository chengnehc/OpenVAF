use std::path::Path;

use basedb::diagnostics::{sink, ConsoleSink, DiagnosticSink};
use basedb::{BaseDB, SourceDatabase, VfsPath, VfsStorage};
use syntax::{Parse, SourceFile};

use expect_test::expect_file;
use mini_harness::{harness, Result};
use parking_lot::RwLock;
use stdx::{ignore_dev_tests, ignore_never, is_va_file, openvaf_test_data, project_root};
use vfs::{AbsPathBuf, FileId, Vfs, VfsEntry};

#[derive(Default)]
#[salsa::database(SourceDatabase)]
pub struct TestDataBase {
    storage: salsa::Storage<TestDataBase>,
    vfs: Option<RwLock<Vfs>>,
    root_file: Option<FileId>,
}

impl TestDataBase {
    pub fn new(path: VfsPath, contents: VfsEntry) -> Self {
        let mut res = Self::default();
        let db: &mut dyn BaseDB = &mut res;
        let vfs = RwLock::new(Vfs::default());
        let root_file = db.setup_test_db(path, contents, &mut vfs.write());
        res.vfs = Some(vfs);
        res.root_file = Some(root_file);
        res
    }

    pub fn new_from_fs(path: &Path) -> Self {
        let path = AbsPathBuf::assert(path.canonicalize().unwrap());
        let contents = std::fs::read(&path);
        TestDataBase::new(path.into(), contents.into())
    }

    pub fn root_file(&self) -> FileId {
        self.root_file.unwrap()
    }

    pub fn parse_and_check(&self) -> (Parse<SourceFile>, String) {
        let root_file = self.root_file();
        let preprocess = self.preprocess(root_file);
        let parse = self.parse(root_file);
        let attr_tree = self.lint_attr_tree(root_file);

        let mut buf = sink::Buffer::no_color();
        {
            let mut sink = ConsoleSink::buffer(self, &mut buf);
            sink.annonymize_paths();
            sink.add_diagnostics(preprocess.errors(), root_file, self);
            sink.add_diagnostics(parse.errors().as_slice(), root_file, self);
            sink.add_diagnostics(attr_tree.diagnostics.as_slice(), root_file, self);
        }
        let data = buf.into_inner();
        let diagnostics = String::from_utf8(data).unwrap();

        (parse, diagnostics)
    }
}

/// This impl tells salsa where to find the salsa runtime.
impl salsa::Database for TestDataBase {}

impl VfsStorage for TestDataBase {
    fn vfs(&self) -> &RwLock<Vfs> {
        self.vfs.as_ref().unwrap()
    }
}

fn integration(dir: &Path) -> Result {
    let name = dir.file_name().unwrap().to_str().unwrap().to_lowercase();
    let main_file = dir.join(format!("{name}.va"));
    let db = TestDataBase::new_from_fs(&main_file);
    let (_, diagnostics) = db.parse_and_check();

    expect_file![dir.join("parser_diagnostics.log")].assert_eq(&diagnostics);

    Ok(())
}

fn syntax_ui(file: &Path) -> Result {
    let db = TestDataBase::new_from_fs(file);
    let (_, diagnostics) = db.parse_and_check();

    expect_file![file.with_extension("log")].assert_eq(&diagnostics);

    Ok(())
}

fn ast(file: &Path) -> Result {
    let db = TestDataBase::new_from_fs(file);
    let (parse, _) = db.parse_and_check();
    let actual = parse.debug_dump();

    expect_file![file.with_extension("vast")].assert_eq(&actual);

    Ok(())
}

harness! {
    Test::from_dir_filtered("integration", &integration, &Path::is_dir, &ignore_dev_tests, &project_root().join("integration_tests")),
    Test::from_dir_filtered("syntax_ui", &syntax_ui, &is_va_file, &ignore_never, &openvaf_test_data("syntax")),
    Test::from_dir_filtered("ast_ok", &ast, &is_va_file, &ignore_never, &openvaf_test_data("ast/ok")),
    Test::from_dir_filtered("ast_err", &ast, &is_va_file, &ignore_never, &openvaf_test_data("ast/err"))
}
