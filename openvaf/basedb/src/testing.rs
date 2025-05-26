use std::mem::transmute;
use std::sync::Arc;

use super::{
    BaseDB, FileId, FileTextQuery, Lint, LintLevel, TiSlice, Vfs, VfsEntry, VfsPath,
    PREDEFINED_MACROS,
};

impl dyn BaseDB {
    pub fn setup_test_db(
        &mut self,
        root_file_path: VfsPath,
        root_file_contents: VfsEntry,
        vfs: &mut Vfs,
    ) -> FileId {
        let root_file = vfs.ensure_file_id(root_file_path);
        vfs.set_file_contents(root_file, root_file_contents);
        vfs.insert_std_lib();

        let include_dirs = vec![VfsPath::new_virtual_path("/std".to_owned())];
        self.set_include_dirs(root_file, Arc::from(include_dirs));

        let macro_flags: Vec<_> = PREDEFINED_MACROS.into_iter().map(Arc::from).collect();
        self.set_macro_flags(root_file, Arc::from(macro_flags));

        self.set_plugin_lints(&[]);

        let overwrites: Arc<[_]> = Arc::from(self.empty_global_lint_overwrites().as_ref());
        let overwrites = unsafe {
            transmute::<Arc<[Option<LintLevel>]>, Arc<TiSlice<Lint, Option<LintLevel>>>>(overwrites)
        };
        self.set_global_lint_overwrites(root_file, overwrites);

        root_file
    }

    pub fn apply_vfs_changes(&mut self) {
        let changes = self.vfs().write().take_changes();
        for change in changes {
            self.invalidate_file(change.file_id);
        }
    }

    pub fn invalidate_file(&mut self, file: FileId) {
        FileTextQuery.in_db_mut(self).invalidate(&file)
    }
}
