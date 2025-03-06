use std::path::Path;

use basedb::diagnostics::sink::Buffer;
use basedb::diagnostics::{ConsoleSink, DiagnosticSink};
use basedb::{AbsPathBuf, BaseDB, FileId, SourceDatabase, Vfs, VfsEntry, VfsPath, VfsStorage};
use hir_def::db::{HirDefDB, HirDefDatabase, InternDatabase};
use hir_def::nameres::{DefMap, LocalScopeId, ScopeItemDef, ScopeOrigin};
use hir_def::DefWithBodyId;
use parking_lot::RwLock;

use expect_test::expect_file;
use mini_harness::{harness, Result};
use stdx::Upcast;
use stdx::{ignore_dev_tests, ignore_never, is_va_file, openvaf_test_data, project_root};

#[salsa::database(SourceDatabase, InternDatabase, HirDefDatabase)]
pub struct TestDataBase {
    storage: salsa::Storage<TestDataBase>,
    vfs: Option<RwLock<Vfs>>,
    root_file: Option<FileId>,
}

impl TestDataBase {
    pub fn new(root_file_path: VfsPath, root_file_contents: VfsEntry) -> Self {
        let vfs = RwLock::new(Vfs::default());
        let mut res = Self { storage: salsa::Storage::default(), vfs: None, root_file: None };
        let db: &mut dyn BaseDB = &mut res;
        let root_file = db.setup_test_db(root_file_path, root_file_contents, &mut vfs.write());
        res.root_file = Some(root_file);
        res.vfs = Some(vfs);
        res
    }

    pub fn new_from_fs(path: &Path) -> Self {
        let path = AbsPathBuf::assert(path.canonicalize().unwrap());
        let file_contents = std::fs::read(&path);
        TestDataBase::new(path.into(), file_contents.into())
    }

    pub fn root_file(&self) -> FileId {
        self.root_file.unwrap()
    }

    pub fn vfs(&self) -> &RwLock<Vfs> {
        self.vfs.as_ref().unwrap()
    }

    pub fn lower_and_check(&self) -> String {
        let root_file = self.root_file();
        let root_def_map = self.root_def_map(root_file);
        let mut buf = Buffer::no_color();
        {
            let mut sink = ConsoleSink::buffer(self, &mut buf);
            sink.annonymize_paths();
            let root_scope = root_def_map.entry_scope();
            self.lower_and_check_rec(root_scope, &root_def_map, &mut sink);
        }
        let data = buf.into_inner();
        String::from_utf8(data).unwrap()
    }

    fn lower_and_check_rec(&self, scope: LocalScopeId, def_map: &DefMap, sink: &mut ConsoleSink) {
        let root_file = self.root_file();

        for (_, declaration) in &def_map[scope].declarations {
            if let Ok(id) = (*declaration).try_into() {
                let diagnostics = &self.body_source_map(id).diagnostics;
                sink.add_diagnostics(diagnostics, root_file, self);
            }
            if let ScopeItemDef::FunctionId(fun) = *declaration {
                let def_map = self.function_def_map(fun);
                let entry = self.function_def_map(fun).entry_scope();
                self.lower_and_check_rec(entry, &def_map, sink)
            }
        }

        for (_, child) in &def_map[scope].children {
            self.lower_and_check_rec(*child, def_map, sink)
        }
    }
}

/// This impl tells salsa where to find the salsa runtime.
impl salsa::Database for TestDataBase {}

impl VfsStorage for TestDataBase {
    fn vfs(&self) -> &RwLock<Vfs> {
        self.vfs()
    }
}

impl Upcast<dyn BaseDB> for TestDataBase {
    fn upcast(&self) -> &(dyn BaseDB + 'static) {
        self
    }
}

fn integration(dir: &Path) -> Result {
    let name = dir.file_name().unwrap().to_str().unwrap().to_lowercase();
    let root_file = dir.join(format!("{name}.va"));
    let db = TestDataBase::new_from_fs(&root_file);
    let diagnostics = db.lower_and_check();

    expect_file![dir.join("hir_def.log")].assert_eq(&diagnostics);

    Ok(())
}

fn body(file: &Path) -> Result {
    let db = TestDataBase::new_from_fs(file);
    let def_map = db.root_def_map(db.root_file());

    let mut actual = String::new();
    for (_, scope) in &def_map[def_map.entry_scope()].children {
        if let ScopeOrigin::Module(module) = def_map[*scope].origin {
            let analog_block = DefWithBodyId::ModuleId { initial: false, module };
            actual.push_str(&db.body(analog_block).dump(&db)?);
            for (_, scope) in &def_map[*scope].children {
                if let ScopeOrigin::Function(func) = def_map[*scope].origin {
                    actual.push_str(&db.body(func.into()).dump(&db)?)
                }
            }
        }
    }

    // std::fs::write(file.with_extension("body"), actual)?;
    expect_file![file.with_extension("body")].assert_eq(&actual);

    Ok(())
}

fn item_tree(file: &Path) -> Result {
    let db = TestDataBase::new_from_fs(file);
    let actual = db.item_tree(db.root_file()).dump()?;

    // std::fs::write(file.with_extension("item_tree"), actual)?;
    expect_file![file.with_extension("item_tree")].assert_eq(&actual);

    Ok(())
}

fn def_map(file: &Path) -> Result {
    let db = TestDataBase::new_from_fs(file);
    let actual = db.root_def_map(db.root_file()).dump(&db)?;

    // std::fs::write(file.with_extension("def_map"), actual)?;
    expect_file![file.with_extension("def_map")].assert_eq(&actual);

    Ok(())
}

harness! {
    Test::from_dir_filtered("integration", &integration, &Path::is_dir, &ignore_dev_tests, &project_root().join("integration_tests")),
    Test::from_dir_filtered("body", &body, &is_va_file, &ignore_never, &openvaf_test_data("body")),
    Test::from_dir_filtered("item_tree", &item_tree, &is_va_file, &ignore_never, &openvaf_test_data("item_tree")),
    Test::from_dir_filtered("def_map", &def_map, &is_va_file, &ignore_never, &openvaf_test_data("item_tree"))
}
