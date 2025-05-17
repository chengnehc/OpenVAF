//！ Name resolution

use std::ops::{Index, IndexMut};
use std::sync::Arc;
use stdx::impl_from;

use arena::{Arena, Idx};
use basedb::FileId;
use indexmap::IndexMap;
use syntax::Name;

use crate::db::HirDefDB;
use crate::{BlockId, BranchId, FunctionId, Lookup, Path};

mod collect;
mod diagnostics;
mod pathres;
mod pretty;
mod scope;

use diagnostics::DefDiagnostic;
use scope::{FlatScope, RecScope, Scope};

pub use diagnostics::{DefDiagnosticWrapped, PathResolveError};
pub use pathres::ResolvedPath;
pub use scope::{ItemWithBodyId, ScopeItem, ScopeItemKind, ScopeOrigin};

/// Definition map.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct DefMap {
    /// where does this `DefMap` come from?
    src: DefMapSource,
    /// all scopes within this `DefMap`
    scopes: Arena<Scope>,
    /// the parent of all scopes. It can be referenced by `$root`
    root_scope: Idx<Scope>,

    pub diagnostics: Vec<DefDiagnostic>,
}

pub type LocalScopeId = Idx<Scope>;

impl Index<LocalScopeId> for DefMap {
    type Output = Scope;
    fn index(&self, index: LocalScopeId) -> &Self::Output {
        &self.scopes[index]
    }
}
impl IndexMut<LocalScopeId> for DefMap {
    fn index_mut(&mut self, index: LocalScopeId) -> &mut Self::Output {
        &mut self.scopes[index]
    }
}

impl DefMap {
    /// The first local scope within this `DefMap`.
    #[inline(always)]
    pub fn entry_scope(&self) -> LocalScopeId {
        LocalScopeId::from(0u32)
    }

    /// The root scope of the entire Verilog-A source file.
    #[inline(always)]
    pub fn root_scope(&self) -> LocalScopeId {
        self.root_scope
    }

    /// Declare a visible scope within this `DefMap`.
    #[inline(always)]
    pub fn declare_scope(
        &mut self,
        origin: ScopeOrigin,
        parent: Option<LocalScopeId>,
    ) -> LocalScopeId {
        let scope = match origin {
            ScopeOrigin::Root | ScopeOrigin::Module(_) | ScopeOrigin::Block(_) => RecScope {
                origin,
                parent,
                children: IndexMap::default(),
                declarations: IndexMap::default(),
            }
            .into(),
            ScopeOrigin::Function(_) => {
                FlatScope { origin, parent, declarations: IndexMap::default() }.into()
            }
        };

        self.scopes.push_and_get_key(scope)
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Copy, Hash)]
pub enum DefMapSource {
    Root,                 // top-level items: natures, disciplines and modules
    Block(BlockId),       // named blocks
    Function(FunctionId), // analog functions
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct ScopeId {
    pub root_file: FileId,
    /// Which kind of `DefMap` does this scope belong to?
    pub src: DefMapSource,
    /// The scope's local ID to the `DefMap`
    pub id: LocalScopeId,
}

impl ScopeId {
    pub fn from(root_file: FileId, src: DefMapSource, id: LocalScopeId) -> Self {
        Self { root_file, src, id }
    }

    pub fn root(root_file: FileId) -> Self {
        Self { root_file, src: DefMapSource::Root, id: 0usize.into() }
    }

    pub fn def_map(&self, db: &dyn HirDefDB) -> Arc<DefMap> {
        match self.src {
            DefMapSource::Root => db.root_def_map(self.root_file),
            DefMapSource::Block(block) => db.block_def_map(block).unwrap(),
            DefMapSource::Function(fun) => db.function_def_map(fun),
        }
    }
}

impl DefMap {
    pub fn resolve_item_name<T: ScopeItemKind>(
        &self,
        scope: LocalScopeId,
        name: &Name,
    ) -> Result<T, PathResolveError> {
        let item = self.resolve_name(scope, name)?;
        item.try_into().map_err(|_| PathResolveError::ExpectedItemKind {
            name: name.clone(),
            expected: T::NAME,
            found: item.into(),
        })
    }

    fn resolve_name(
        &self,
        scope: LocalScopeId,
        name: &Name,
    ) -> Result<ScopeItem, PathResolveError> {
        let mut cur_scope = scope;
        loop {
            if let Some(decl) = self[cur_scope].decls().get(name) {
                return Ok(*decl);
            }
            let Some(parent) = self[cur_scope].parent() else {
                return Err(PathResolveError::NotFound { name: name.clone() });
            };
            cur_scope = parent;
        }
    }
}

impl ScopeId {
    pub fn resolve_path(
        &self,
        db: &dyn HirDefDB,
        path: &Path,
    ) -> Result<ResolvedPath, PathResolveError> {
        match self.src {
            DefMapSource::Block(_) if path.is_root => {
                db.root_def_map(self.root_file).resolve_root_path(&path.segments, db)
            }
            DefMapSource::Function(_) | DefMapSource::Root if path.is_root => {
                self.def_map(db).resolve_root_path(&path.segments, db)
            }
            _ => self.def_map(db).resolve_normal_path(self.id, &path.segments, db),
        }
    }

    pub fn resolve_item_path<T: ScopeItemKind>(
        &self,
        db: &dyn HirDefDB,
        path: &Path,
    ) -> Result<T, PathResolveError> {
        match self.src {
            DefMapSource::Block(_) if path.is_root => {
                db.root_def_map(self.root_file).resolve_root_item_path(&path.segments, db)
            }
            DefMapSource::Function(_) | DefMapSource::Root if path.is_root => {
                self.def_map(db).resolve_root_item_path(&path.segments, db)
            }
            _ => self.def_map(db).resolve_normal_item_path(self.id, &path.segments, db),
        }
    }

    pub fn resolve_item_name<T: ScopeItemKind>(
        &self,
        db: &dyn HirDefDB,
        name: &Name,
    ) -> Result<T, PathResolveError> {
        self.def_map(db).resolve_item_name(self.id, name)
    }

    pub fn resolve_name(
        &self,
        db: &dyn HirDefDB,
        name: &Name,
    ) -> Result<ScopeItem, PathResolveError> {
        let def_map = self.def_map(db);
        let mut cur_scope = self.id;
        loop {
            if let Some(decl) = def_map[cur_scope].decls().get(name) {
                return Ok(*decl);
            }
            let Some(parent) = def_map[cur_scope].parent() else {
                return Err(PathResolveError::NotFound { name: name.clone() });
            };
            cur_scope = parent;
        }
    }
}
