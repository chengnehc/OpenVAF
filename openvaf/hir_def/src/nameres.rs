//！ Name resolution

use std::ops::{Index, IndexMut};
use std::sync::Arc;
use stdx::{impl_display, impl_from, impl_from_typed};

use arena::{Arena, Idx};
use basedb::{AstIdMap, /*ErasedAstId,*/ FileId};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use syntax::name::{kw, Name};
use syntax::{AstNode, Parse, SourceFile, TextRange};

use crate::builtin::{insert_builtin_scope, BuiltIn, ParamSysFun};
use crate::db::HirDefDB;
use crate::{
    AliasParamId, BlockId, BranchId, DisciplineId, FunctionArgId, FunctionId, Lookup, ModuleId,
    NatureAttrId, NatureId, NodeId, ParamId, VarId,
};

mod collect;
mod diagnostics;
mod pretty;

use diagnostics::DefDiagnostic;
pub use diagnostics::{DefDiagnosticWrapped, PathResolveError};

/// Contains the results of name resolution.
#[derive(PartialEq, Eq, Clone, Debug)]
pub struct DefMap {
    /// where does this `DefMap` come from?
    src: DefMapSource,
    /// all scopes within this `DefMap`
    scopes: Arena<ScopeData>,
    /// the parent of all other scopes, defined by `$root`
    root_id: Idx<ScopeData>,

    pub diagnostics: Vec<DefDiagnostic>,
}

/// An ID of a scope, **local** to a `DefMap`.
pub type LocalScopeId = Idx<ScopeData>;

impl Index<LocalScopeId> for DefMap {
    type Output = ScopeData;
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
    /// the first local scope within this `DefMap`
    #[inline(always)]
    pub fn entry_scope(&self) -> LocalScopeId {
        LocalScopeId::from(0u32)
    }

    #[inline(always)]
    pub fn root_scope(&self) -> LocalScopeId {
        self.root_id
    }

    pub fn new_scope(&mut self, origin: ScopeOrigin, parent: LocalScopeId) -> LocalScopeId {
        self.scopes.push_and_get_key(ScopeData {
            origin,
            parent: Some(parent),
            children: IndexMap::default(),
            declarations: IndexMap::default(),
        })
    }

    pub fn new_root_scope(&mut self, origin: ScopeOrigin) -> LocalScopeId {
        self.scopes.push_and_get_key(ScopeData {
            origin,
            parent: None,
            children: IndexMap::default(),
            declarations: IndexMap::default(),
        })
    }
}

#[derive(PartialEq, Eq, Clone, Debug, Copy, Hash)]
pub enum DefMapSource {
    Root,
    Block(BlockId),
    Function(FunctionId),
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct ScopeData {
    /// where does this `Scope` come from?
    pub origin: ScopeOrigin,
    /// parent scope, `None` for root scope
    pub parent: Option<LocalScopeId>,
    /// children scopes defined within this scope
    pub children: IndexMap<Name, LocalScopeId, ahash::RandomState>,
    /// items declared in this scope
    pub declarations: IndexMap<Name, ScopeItemDef, ahash::RandomState>,
}

#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum ScopeOrigin {
    Root,
    Module(ModuleId),
    // Nature(NatureId),
    // Discipline(DisciplineId),
    Block(BlockId),
    Function(FunctionId),
}

impl_from_typed! {
    Module(ModuleId),
    // Nature(NatureId),
    // Discipline(DisciplineId),
    Block(BlockId),
    Function(FunctionId)

    for ScopeOrigin
}

// nature access is a special kind of nature attribute
#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub struct NatureAccess(pub NatureAttrId);

impl From<NatureAttrId> for NatureAccess {
    fn from(id: NatureAttrId) -> NatureAccess {
        NatureAccess(id)
    }
}

/// Items that can be defined within a scope.
#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub enum ScopeItemDef {
    ModuleId(ModuleId),
    NatureId(NatureId),
    NatureAttrId(NatureAttrId),
    NatureAccess(NatureAccess),
    DisciplineId(DisciplineId),
    BlockId(BlockId),
    NodeId(NodeId),
    BranchId(BranchId),
    VarId(VarId),
    ParamId(ParamId),
    ParamSysFun(ParamSysFun),
    AliasParamId(AliasParamId),
    BuiltIn(BuiltIn),
    FunctionId(FunctionId),
    FunctionArgId(FunctionArgId),
    FunctionReturn(FunctionId),
}

impl ScopeItemDef {
    /*
    pub fn ast_id(&self, db: &dyn HirDefDB) -> Option<ErasedAstId> {
        let id: ErasedAstId = match self {
            ScopeDefItem::ModuleId(module) => module.lookup(db).ast_id(db).into(),
            ScopeDefItem::BlockId(block) => block.lookup(db).ast.into(),
            ScopeDefItem::NatureId(nature) => nature.lookup(db).ast_id(db).into(),
            ScopeDefItem::NatureAccess(access) => access.0.lookup(db).ast_id(db).into(),
            ScopeDefItem::DisciplineId(discipline) => discipline.lookup(db).ast_id(db).into(),
            ScopeDefItem::VarId(var) => var.lookup(db).ast_id(db).into(),
            ScopeDefItem::ParamId(param) => param.lookup(db).ast_id(db).into(),
            ScopeDefItem::BranchId(branch) => branch.lookup(db).ast_id(db).into(),
            ScopeDefItem::FunctionReturn(fun) | ScopeDefItem::FunctionId(fun) => {
                fun.lookup(db).ast_id(db).into()
            }
            ScopeDefItem::FunctionArgId(arg) => arg.lookup(db).ast_id(db).into(),
            ScopeDefItem::NodeId(node) => node.lookup(db).ast_id(db),
            ScopeDefItem::BuiltIn(_) | ScopeDefItem::ParamSysFun(_) => return None,
            ScopeDefItem::AliasParamId(id) => id.lookup(db).ast_id(db).into(),
            ScopeDefItem::NatureAttrId(id) => id.lookup(db).ast_id(db).into(),
        };
        Some(id)
    }
    */

    pub fn text_range(
        &self,
        db: &dyn HirDefDB,
        ast_id_map: &AstIdMap,
        parse: &Parse<SourceFile>,
    ) -> Option<TextRange> {
        let res = match self {
            ScopeItemDef::ModuleId(module) => ast_id_map
                .get(module.lookup(db).ast_id(db))
                .to_node(parse.tree().syntax())
                .name()?
                .syntax()
                .text_range(),
            ScopeItemDef::BlockId(block) => ast_id_map
                .get(block.lookup(db).ast_id)
                .to_node(parse.tree().syntax())
                .block_scope()?
                .name()?
                .syntax()
                .text_range(),
            ScopeItemDef::NatureId(nature) => ast_id_map
                .get(nature.lookup(db).ast_id(db))
                .to_node(parse.tree().syntax())
                .name()?
                .syntax()
                .text_range(),
            ScopeItemDef::NatureAccess(access) => {
                ast_id_map.get(access.0.lookup(db).ast_id(db)).text_range()
            }
            ScopeItemDef::DisciplineId(discipline) => ast_id_map
                .get(discipline.lookup(db).ast_id(db))
                .to_node(parse.tree().syntax())
                .name()?
                .syntax()
                .text_range(),
            ScopeItemDef::VarId(var) => ast_id_map.get(var.lookup(db).ast_id(db)).text_range(),
            ScopeItemDef::ParamId(param) => {
                ast_id_map.get(param.lookup(db).ast_id(db)).text_range()
            }
            ScopeItemDef::BranchId(branch) => {
                let branch = branch.lookup(db);
                let pos = branch.item_tree(db)[branch.id].name_idx;
                ast_id_map
                    .get(branch.ast_id(db))
                    .to_node(parse.tree().syntax())
                    .names()
                    .nth(pos)?
                    .syntax()
                    .text_range()
            }
            ScopeItemDef::FunctionReturn(fun) | ScopeItemDef::FunctionId(fun) => ast_id_map
                .get(fun.lookup(db).ast_id(db))
                .to_node(parse.tree().syntax())
                .name()?
                .syntax()
                .text_range(),
            ScopeItemDef::FunctionArgId(arg) => {
                let arg = arg.lookup(db);
                let fun = arg.fun.lookup(db);
                let pos = fun.item_tree(db)[fun.id].args[arg.id].name_idx;
                ast_id_map
                    .get(arg.ast_id(db))
                    .to_node(parse.tree().syntax())
                    .names()
                    .nth(pos)?
                    .syntax()
                    .text_range()
            }
            ScopeItemDef::NodeId(node) => {
                ast_id_map.get_erased(node.lookup(db).ast_id(db)).text_range()
            }
            ScopeItemDef::BuiltIn(_) | ScopeItemDef::ParamSysFun(_) => return None,
            ScopeItemDef::AliasParamId(id) => ast_id_map.get(id.lookup(db).ast_id(db)).text_range(),
            ScopeItemDef::NatureAttrId(id) => ast_id_map.get(id.lookup(db).ast_id(db)).text_range(),
        };

        Some(res)
    }
}

impl_from! {
    ModuleId,
    NatureId,
    NatureAttrId,
    NatureAccess,
    DisciplineId,
    // DisciplineAttrId,
    BlockId,
    NodeId,
    BranchId,
    VarId,
    ParamId,
    AliasParamId,
    ParamSysFun,
    BuiltIn,
    FunctionId,
    FunctionArgId
    // FunctionReturn

    for ScopeItemDef
}

pub trait ScopeItemKind: TryFrom<ScopeItemDef> {
    const NAME: &'static str;
}

macro_rules! scope_item_kinds {
    ($($ty: ident => $name:literal),*) => {
        $(impl ScopeItemKind for $ty{const NAME: &'static str = $name;})*
        impl ScopeItemDef {
            pub const fn item_kind(&self) -> &'static str {
                match self {
                    $(ScopeItemDef::$ty(_) => $ty::NAME,)*
                    ScopeItemDef::FunctionReturn(_) => VarId::NAME
                }
            }
        }

    };
}

scope_item_kinds! {
    ModuleId => "module",
    NatureId => "nature",
    NatureAttrId => "nature attribute",
    NatureAccess => "nature access function",
    DisciplineId => "discipline",
    BlockId => "block scope",
    NodeId => "node",
    BranchId => "branch",
    VarId => "variable",
    ParamId => "parameter",
    AliasParamId => "parameter",
    ParamSysFun => "hierarchical parameter system function",
    BuiltIn => "function",
    FunctionId => "function",
    FunctionArgId => "function argument"
}

/// DefMap queries
impl DefMap {
    pub fn root_def_map_query(db: &dyn HirDefDB, root_file: FileId) -> Arc<DefMap> {
        collect::collect_root_def_map(db, root_file)
    }

    pub fn block_def_map_query(db: &dyn HirDefDB, block: BlockId) -> Option<Arc<DefMap>> {
        collect::collect_block_map(db, block)
    }

    pub fn function_def_map_query(db: &dyn HirDefDB, fun: FunctionId) -> Arc<DefMap> {
        collect::collect_function_map(db, fun)
    }
}

static BUILTIN_SCOPE: Lazy<IndexMap<Name, ScopeItemDef, ahash::RandomState>> = Lazy::new(|| {
    let mut scope = IndexMap::default();
    insert_builtin_scope(&mut scope);
    scope
});

/// Name resolution algorithms
impl DefMap {
    pub fn resolve_local_name_in_scope(
        &self,
        mut scope: LocalScopeId,
        name: &Name,
    ) -> Result<ScopeItemDef, PathResolveError> {
        loop {
            if let Some(decl) = self.scopes[scope].declarations.get(name) {
                return Ok(*decl);
            }

            match self[scope].parent {
                Some(parent) => scope = parent,
                None => return Err(PathResolveError::NotFound { name: name.clone() }),
            }
        }
    }

    pub fn resolve_local_item_in_scope<T: ScopeItemKind>(
        &self,
        scope: LocalScopeId,
        name: &Name,
    ) -> Result<T, PathResolveError> {
        let res = self.resolve_local_name_in_scope(scope, name)?;
        res.try_into().map_err(|_| PathResolveError::ExpectedItemKind {
            name: name.clone(),
            expected: T::NAME,
            found: res.into(),
        })
    }

    pub fn resolve_normal_path_in_scope(
        &self,
        mut scope: LocalScopeId,
        path: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        let mut arc;
        let mut current_map = self;

        let name = &path[0];

        let decl = loop {
            if let Some(decl) = current_map.scopes[scope].declarations.get(name) {
                break *decl;
            }

            match current_map[scope].parent {
                Some(parent) => scope = parent,
                None => match current_map.src {
                    DefMapSource::Block(block) => {
                        let block = block.lookup(db);
                        arc = block.parent.def_map(db);
                        current_map = &*arc;
                        scope = block.parent.local_id;
                    }
                    DefMapSource::Root => {
                        if let Some(builtin) = BUILTIN_SCOPE.get(name) {
                            break *builtin;
                        }

                        return Err(PathResolveError::NotFound { name: name.clone() });
                    }
                    DefMapSource::Function(_fun) => {
                        if let Some(builtin) = BUILTIN_SCOPE.get(name) {
                            break *builtin;
                        }

                        // let parent = fun.lookup(db).scope;
                        //parent.def_map(db).resolve_normal_path_in_scope(scope, path, db);
                        // TODO give hint if found in full def map

                        return Err(PathResolveError::NotFound { name: name.clone() });
                    }
                },
            }
        };

        if path.len() == 1 {
            return Ok(decl.into());
        }

        scope = match decl {
            ScopeItemDef::ModuleId(module) => module.lookup(db).scope.local_id,

            ScopeItemDef::BlockId(block) => match db.block_def_map(block) {
                Some(block_map) => {
                    arc = block_map;
                    current_map = &*arc;
                    current_map.entry_scope()
                }
                None => {
                    return Err(PathResolveError::NotFoundIn {
                        name: path[1].clone(),
                        scope: path[0].clone(),
                    })
                }
            },

            _ => return Err(PathResolveError::ExpectedScope { name: name.clone(), found: decl }),
        };

        current_map.resolve_names_in(scope, name, &path[1..], db)
    }

    pub fn resolve_normal_item_path_in_scope<T: ScopeItemKind>(
        &self,
        scope: LocalScopeId,
        path: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<T, PathResolveError> {
        let resolved_path = self.resolve_normal_path_in_scope(scope, path, db)?;
        let res: Result<ScopeItemDef, _> = resolved_path.clone().try_into();
        if let Ok(res) = res {
            if let Ok(res) = res.try_into() {
                return Ok(res);
            }
        }
        Err(PathResolveError::ExpectedItemKind {
            name: path.last().unwrap().clone(),
            expected: T::NAME,
            found: resolved_path,
        })
    }

    /// Resolve path with `$root` prefix. Refer to LRM chapter 6.2
    pub fn resolve_root_path(
        &self,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        debug_assert!(matches!(self.src, DefMapSource::Function(_) | DefMapSource::Root));
        self.resolve_names_in(self.root_scope(), &kw::root, segments, db)
    }

    pub fn resolve_root_item_path<T: ScopeItemKind>(
        &self,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<T, PathResolveError> {
        let resolved_path = self.resolve_root_path(segments, db)?;
        let res: Result<ScopeItemDef, _> = resolved_path.clone().try_into();
        if let Ok(res) = res {
            if let Ok(res) = res.try_into() {
                return Ok(res);
            }
        }
        Err(PathResolveError::ExpectedItemKind {
            name: segments.last().unwrap().clone(),
            expected: T::NAME,
            found: resolved_path,
        })
    }

    fn resolve_names_in<'a>(
        &self,
        mut scope: LocalScopeId,
        mut scope_name: &'a Name,
        path: &'a [Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        let (segments, name) =
            if let [segments @ .., name] = path { (segments, name) } else { unreachable!() };

        let mut arc;
        let mut current_map = self;

        for (i, segment) in segments.iter().enumerate() {
            match current_map.scopes[scope].children.get(segment) {
                Some(child) => scope = *child,
                None => match current_map.scopes[scope].declarations.get(segment) {
                    Some(ScopeItemDef::BlockId(block)) => match db.block_def_map(*block) {
                        Some(block_map) => {
                            arc = block_map;
                            current_map = &*arc;
                            scope = current_map.entry_scope();
                        }
                        None => {
                            return Err(PathResolveError::NotFoundIn {
                                name: path[i + 1].clone(),
                                scope: segment.clone(),
                            })
                        }
                    },
                    Some(ScopeItemDef::BranchId(branch))
                        if path.get(i + 1) == Some(&kw::potential) =>
                    {
                        let rem = &path[(i + 1)..];
                        if let [name] = rem {
                            return Ok(ResolvedPath::PotentialAttribute {
                                branch: *branch,
                                name: name.clone(),
                            });
                        } else {
                            return Err(PathResolveError::ExpectedNatureAttrIdent {
                                found: rem.to_owned().into_boxed_slice(),
                            });
                        }
                    }

                    Some(ScopeItemDef::BranchId(branch)) if path.get(i + 1) == Some(&kw::flow) => {
                        let rem = &path[(i + 1)..];
                        if let [name] = rem {
                            return Ok(ResolvedPath::FlowAttribute {
                                branch: *branch,
                                name: name.clone(),
                            });
                        } else {
                            return Err(PathResolveError::ExpectedNatureAttrIdent {
                                found: rem.to_owned().into_boxed_slice(),
                            });
                        }
                    }

                    Some(found) => {
                        return Err(PathResolveError::ExpectedScope {
                            name: segment.clone(),
                            found: *found,
                        });
                    }
                    None => {
                        return Err(PathResolveError::NotFoundIn {
                            name: segment.clone(),
                            scope: scope_name.clone(),
                        })
                    }
                },
            }

            scope_name = segment;
        }

        match self.scopes[scope].declarations.get(name) {
            Some(res) => Ok(ResolvedPath::ScopeItemDef(*res)),
            None => {
                Err(PathResolveError::NotFoundIn { name: name.clone(), scope: scope_name.clone() })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPath {
    FlowAttribute { branch: BranchId, name: Name },
    PotentialAttribute { branch: BranchId, name: Name },
    ScopeItemDef(ScopeItemDef),
}

impl_display! {
    match ResolvedPath{
        ResolvedPath::FlowAttribute{..}  => "nature attribute";
        ResolvedPath::PotentialAttribute{..} => "nature attribute";
        ResolvedPath::ScopeItemDef(item) => "{}", item.item_kind();
    }
}

impl_from!(ScopeItemDef for ResolvedPath);
