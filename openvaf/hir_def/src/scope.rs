use std::sync::Arc;

use basedb::FileId;

use crate::db::HirDefDB;
use crate::nameres::{
    DefMap, DefMapSource, LocalScopeId, PathResolveError, ResolvedPath, ScopeItemDef, ScopeItemKind,
};
use crate::{Name, Path};

/// A  representation of a scope.
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct Scope {
    pub root_file: FileId,
    /// Which kind of `DefMap` does this scope belong to?
    pub src: DefMapSource,
    /// The scope's **local** ID to the `DefMap`
    pub local_id: LocalScopeId,
}

impl Scope {
    pub fn from(root_file: FileId, src: DefMapSource, local_id: LocalScopeId) -> Self {
        Self { root_file, src, local_id }
    }

    pub fn root(root_file: FileId) -> Self {
        Self { root_file, src: DefMapSource::Root, local_id: 0usize.into() }
    }

    pub fn def_map(&self, db: &dyn HirDefDB) -> Arc<DefMap> {
        match self.src {
            DefMapSource::Root => db.root_def_map(self.root_file),
            DefMapSource::Block(block) => db.block_def_map(block).unwrap(),
            DefMapSource::Function(fun) => db.function_def_map(fun),
        }
    }

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
            _ => self.def_map(db).resolve_normal_path(self.local_id, &path.segments, db),
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
            _ => self.def_map(db).resolve_normal_item_path(self.local_id, &path.segments, db),
        }
    }

    pub fn resolve_item_name<T: ScopeItemKind>(
        &self,
        db: &dyn HirDefDB,
        name: &Name,
    ) -> Result<T, PathResolveError> {
        self.def_map(db).resolve_item_name(self.local_id, name)
    }

    pub fn resolve_name(
        &self,
        db: &dyn HirDefDB,
        name: &Name,
    ) -> Result<ScopeItemDef, PathResolveError> {
        let def_map = self.def_map(db);
        let mut cur_scope = self.local_id;
        loop {
            if let Some(decl) = def_map[cur_scope].declarations.get(name) {
                return Ok(*decl);
            }
            let Some(parent) = def_map[cur_scope].parent else {
                return Err(PathResolveError::NotFound { name: name.clone() });
            };
            cur_scope = parent;
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
    ) -> Result<ScopeItemDef, PathResolveError> {
        let mut cur_scope = scope;
        loop {
            if let Some(decl) = self[cur_scope].declarations.get(name) {
                return Ok(*decl);
            }
            let Some(parent) = self[cur_scope].parent else {
                return Err(PathResolveError::NotFound { name: name.clone() });
            };
            cur_scope = parent;
        }
    }
}
