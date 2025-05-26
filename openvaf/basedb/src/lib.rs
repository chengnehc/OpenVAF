//! See Also:
//! - [rust-analyzer `basedb`](https://github.com/rust-lang/rust-analyzer/tree/master/crates/base-db)

use std::fs;
use std::sync::Arc;

use parking_lot::RwLock;
use syntax::{Parse, Preprocess, SourceFile, SourceMap, SourceProvider, TextRange, TextSize};
use typed_index_collections::{TiSlice, TiVec};

pub use vfs::{AbsPathBuf, FileId, FileReadError, Vfs, VfsEntry, VfsPath};

pub mod diagnostics;
pub mod lints;

use lints::{Lint, LintAttrTree, LintData, LintLevel, LintRegistry};

mod ast_id_map;
mod line_index;
mod testing;

pub use ast_id_map::{AstId, AstIdMap, ErasedAstId};
pub use line_index::{Line, LineIndex};

/// Refer to LRM chapter 10.5
pub const PREDEFINED_MACROS: [&str; 3] =
    ["__OPENVAF__", "__VAMS_ENABLE__", "__VAMS_COMPACT_MODELING__"];

pub trait VfsStorage {
    fn vfs(&self) -> &RwLock<Vfs>;
}

#[salsa::query_group(SourceDatabase)]
pub trait BaseDB: VfsStorage {
    #[salsa::input]
    fn include_dirs(&self, root_file: FileId) -> Arc<[VfsPath]>;
    #[salsa::input]
    fn macro_flags(&self, file_root: FileId) -> Arc<[Arc<str>]>;

    /* preprocess and parse */
    fn preprocess(&self, root_file: FileId) -> Preprocess;
    #[salsa::transparent]
    fn sourcemap(&self, root_file: FileId) -> Arc<SourceMap>;
    fn parse(&self, root_file: FileId) -> Parse<SourceFile>;
    fn ast_id_map(&self, root_file: FileId) -> Arc<AstIdMap>;

    /* line index */
    fn line_index(&self, file_id: FileId) -> Arc<LineIndex>;
    #[salsa::transparent]
    fn line(&self, pos: TextSize, file: FileId) -> Line;
    #[salsa::transparent]
    fn line_range(&self, line: Line, file: FileId) -> TextRange;
    // #[salsa::transparent]
    // fn line_col(&self, span: FileSpan) -> LineCol;

    /* vfs */
    fn file_text(&self, file: FileId) -> Result<Arc<str>, FileReadError>;
    #[salsa::transparent]
    fn file_path(&self, file: FileId) -> VfsPath;
    #[salsa::transparent]
    fn file_id(&self, path: VfsPath) -> FileId;

    /* linting */
    #[salsa::input]
    fn plugin_lints(&self) -> &'static [LintData];
    #[salsa::input]
    fn global_lint_overwrites(&self, root_file: FileId) -> Arc<TiSlice<Lint, Option<LintLevel>>>;
    #[salsa::transparent]
    fn empty_global_lint_overwrites(&self) -> TiVec<Lint, Option<LintLevel>>;
    #[salsa::invoke(LintRegistry::new)]
    fn lint_registry(&self) -> Arc<LintRegistry>;
    #[salsa::invoke(LintAttrTree::query)]
    fn lint_attr_tree(&self, root_file: FileId) -> Arc<LintAttrTree>;
    #[salsa::transparent]
    fn lint(&self, name: &str) -> Option<Lint>;
    #[salsa::transparent]
    fn lint_data(&self, lint: Lint) -> LintData;
    #[salsa::transparent]
    fn lint_lvl(
        &self,
        lint: Lint,
        root_file: FileId,
        sctx: Option<ErasedAstId>,
    ) -> (LintLevel, bool);
}

/* Source Database */

fn preprocess(db: &dyn BaseDB, root_file: FileId) -> Preprocess {
    syntax::preprocess(&db.as_src_provider(), root_file)
}

#[inline]
fn sourcemap(db: &dyn BaseDB, root_file: FileId) -> Arc<SourceMap> {
    db.preprocess(root_file).source_map
}

fn parse(db: &dyn BaseDB, root_file: FileId) -> Parse<SourceFile> {
    let preprocess = &db.preprocess(root_file);
    syntax::parse(&db.as_src_provider(), root_file, preprocess)
}

fn ast_id_map(db: &dyn BaseDB, root_file: FileId) -> Arc<AstIdMap> {
    let ast = db.parse(root_file).root();
    let ast_id_map = AstIdMap::from_source(&ast);
    Arc::new(ast_id_map)
}

/* Line Index */

#[inline]
fn line_index(db: &dyn BaseDB, file_id: FileId) -> Arc<LineIndex> {
    let vfs = db.vfs().read();
    let text = vfs.file_contents_unchecked(file_id);
    Arc::new(LineIndex::new(text))
}

#[inline]
fn line(db: &dyn BaseDB, pos: TextSize, file: FileId) -> Line {
    db.line_index(file).line(pos)
}

#[inline]
fn line_range(db: &dyn BaseDB, line: Line, file: FileId) -> TextRange {
    db.line_index(file).line_range(line)
}

// #[inline]
// fn line_col(db: &dyn BaseDB, span: FileSpan) -> LineCol {
//     db.line_index(span.file).line_col(span.range.start())
// }

/* VFS */

#[inline]
fn file_text(db: &dyn BaseDB, file: FileId) -> Result<Arc<str>, FileReadError> {
    db.salsa_runtime().report_synthetic_read(salsa::Durability::LOW);
    let vfs = db.vfs().read();
    // TODO request file from FS

    match vfs.file_contents(file) {
        Ok(res) => Ok(res.into()),
        Err(err) => {
            if let Some(path) = vfs.file_path(file).as_path() {
                drop(vfs);
                let mut vfs = db.vfs().write();
                vfs.set_file_contents(file, fs::read(path).into());
                vfs.file_contents(file).map(Arc::from)
            } else {
                Err(err)
            }
        }
    }
}

fn file_path(db: &dyn BaseDB, file: FileId) -> VfsPath {
    db.vfs().read().file_path(file)
}

fn file_id(db: &dyn BaseDB, path: VfsPath) -> FileId {
    db.vfs().write().ensure_file_id(path)
}

/* Linting */

fn lint(db: &dyn BaseDB, name: &str) -> Option<Lint> {
    db.lint_registry().lint_from_name(name)
}

fn lint_data(db: &dyn BaseDB, lint: Lint) -> LintData {
    db.lint_registry().lint_data(lint)
}

fn lint_lvl(
    db: &dyn BaseDB,
    lint: Lint,
    root_file: FileId,
    ast: Option<ErasedAstId>,
) -> (LintLevel, bool) {
    if let Some(ast) = ast {
        let map = &db.ast_id_map(root_file);
        if let Some(lvl) = db.lint_attr_tree(root_file).lint_lvl(map, ast, lint) {
            return (lvl, false);
        }
    }
    if let Some(lvl) = db.global_lint_overwrites(root_file)[lint] {
        return (lvl, false);
    }

    (db.lint_data(lint).default_lvl, true)
}

fn empty_global_lint_overwrites(db: &dyn BaseDB) -> TiVec<Lint, Option<LintLevel>> {
    vec![None; db.plugin_lints().len() + lints::builtin::ALL.len()].into()
}

struct SourceProviderDelegate<'a>(&'a dyn BaseDB);

impl dyn BaseDB + '_ {
    pub fn as_src_provider(&self) -> impl SourceProvider + '_ {
        SourceProviderDelegate(self)
    }
}

impl SourceProvider for SourceProviderDelegate<'_> {
    #[inline(always)]
    fn include_dirs(&self, root_file: FileId) -> Arc<[VfsPath]> {
        self.0.include_dirs(root_file)
    }

    #[inline(always)]
    fn macro_flags(&self, root_file: FileId) -> Arc<[Arc<str>]> {
        self.0.macro_flags(root_file)
    }

    #[inline(always)]
    fn file_text(&self, file: FileId) -> Result<Arc<str>, FileReadError> {
        self.0.file_text(file)
    }

    #[inline(always)]
    fn file_path(&self, file: FileId) -> VfsPath {
        self.0.file_path(file)
    }

    #[inline(always)]
    fn file_id(&self, path: VfsPath) -> FileId {
        self.0.file_id(path)
    }
}
