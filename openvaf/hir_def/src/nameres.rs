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
    /// the parent of all scopes. It can be referenced by `$root`
    root_scope: Idx<ScopeData>,

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
        self.root_scope
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
    /// where does this scope originate from?
    pub origin: ScopeOrigin,
    /// parent scope, `None` for root scope
    pub parent: Option<LocalScopeId>,
    /// children scopes defined within this scope
    pub children: IndexMap<Name, LocalScopeId, ahash::RandomState>,
    /// items declared in this scope
    pub declarations: IndexMap<Name, ScopeItemDef, ahash::RandomState>,
}

/// Where does the scope originate from?
///
/// In Verilog-A, only modules, named blocks and analog functions define a new scope.
/// Identifiers defined as natures or disciplines have a global scope (root scope),
/// which allows nets to be declared inside any module in the same manner as an
/// instance of a module.
///
/// See: LRM 3.13 Namespace and 6.8 Scope rules
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum ScopeOrigin {
    Root,
    Module(ModuleId),
    Block(BlockId), // A named block, specifically
    Function(FunctionId),
}
impl_from_typed! {
    Module(ModuleId),
    Block(BlockId),
    Function(FunctionId) for ScopeOrigin
}

/// Nature access function is a special kind of nature attribute.
///
/// Each access function defined before a module is parsed is automatically
/// added to that module’s namespace, unless there is another identifier defined
/// with the same name as the access function in that module’s namespace.
///
/// See: LRM 3.13.2 Access functions
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
        collect::root_def_map(db, root_file)
    }

    pub fn block_def_map_query(db: &dyn HirDefDB, block: BlockId) -> Option<Arc<DefMap>> {
        collect::block_def_map(db, block)
    }

    pub fn function_def_map_query(db: &dyn HirDefDB, fun: FunctionId) -> Arc<DefMap> {
        collect::function_def_map(db, fun)
    }
}

static BUILTIN_SCOPE: Lazy<IndexMap<Name, ScopeItemDef, ahash::RandomState>> = Lazy::new(|| {
    let mut scope = IndexMap::default();
    insert_builtin_scope(&mut scope);
    scope
});

/// Name resolution algorithms
impl DefMap {
    pub fn resolve_item_in<T: ScopeItemKind>(
        &self,
        scope: LocalScopeId,
        name: &Name,
    ) -> Result<T, PathResolveError> {
        let item = self.resolve_name_in(scope, name)?;
        item.try_into().map_err(|_| PathResolveError::ExpectedItemKind {
            name: name.clone(),
            expected: T::NAME,
            found: item.into(),
        })
    }

    fn resolve_name_in(
        &self,
        scope: LocalScopeId,
        name: &Name,
    ) -> Result<ScopeItemDef, PathResolveError> {
        let mut scope = scope;
        loop {
            if let Some(decl) = self[scope].declarations.get(name) {
                return Ok(*decl);
            }
            let Some(parent) = self[scope].parent else {
                return Err(PathResolveError::NotFound { name: name.clone() });
            };
            scope = parent;
        }
    }

    pub fn resolve_normal_item_path_in<T: ScopeItemKind>(
        &self,
        scope: LocalScopeId,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<T, PathResolveError> {
        let resolved_path = self.resolve_normal_path_in(scope, segments, db)?;
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

    pub fn resolve_normal_path_in(
        &self,
        scope: LocalScopeId,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        let mut scope = scope;
        let mut def_map = self;
        let mut arc;
        let name = segments.first().unwrap();
        let decl = loop {
            // try resolving in this scope
            if let Some(decl) = def_map[scope].declarations.get(name) {
                break *decl;
            }
            // try resolving in parent scopes
            match def_map[scope].parent {
                Some(parent) => scope = parent,
                None => match def_map.src {
                    DefMapSource::Block(block) => {
                        // switch to block def map
                        let parent = block.lookup(db).parent;
                        scope = parent.local_id;
                        arc = parent.def_map(db);
                        def_map = &arc;
                    }
                    DefMapSource::Root | DefMapSource::Function(_) => {
                        // TODO give hint if found in full def map
                        let Some(builtin) = BUILTIN_SCOPE.get(name) else {
                            return Err(PathResolveError::NotFound { name: name.clone() });
                        };
                        break *builtin;
                    }
                },
            }
        };

        if segments.len() == 1 {
            return Ok(decl.into());
        }

        scope = match decl {
            ScopeItemDef::ModuleId(module) => module.lookup(db).scope.local_id,
            ScopeItemDef::BlockId(block) => match db.block_def_map(block) {
                Some(block_map) => {
                    arc = block_map;
                    def_map = &arc;
                    def_map.entry_scope()
                }
                None => {
                    return Err(PathResolveError::NotFoundIn {
                        name: segments[1].clone(),
                        scope: segments[0].clone(),
                    })
                }
            },
            _ => return Err(PathResolveError::ExpectedScope { name: name.clone(), found: decl }),
        };

        def_map.resolve_path_in(scope, name, &segments, db)
    }

    /// Resolve a path with `$root` prefix.
    ///
    /// See: LRM chapter 6.2.1
    pub fn resolve_root_path(
        &self,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        // JW: paths in named `Block` should be resolved in the root def map
        // while paths in analog `Function` should be resolved in the function's own def map
        // even if $root is given
        debug_assert!(matches!(self.src, DefMapSource::Root | DefMapSource::Function(_)));
        self.resolve_path_in(self.root_scope(), &kw::root, segments, db)
    }

    pub fn resolve_root_item_path<T: ScopeItemKind>(
        &self,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<T, PathResolveError> {
        let resolved_path = self.resolve_root_path(segments, db)?;
        let res: Result<ScopeItemDef, _> = resolved_path.clone().try_into();
        if let Ok(item) = res {
            if let Ok(res) = item.try_into() {
                return Ok(res);
            }
        }
        Err(PathResolveError::ExpectedItemKind {
            name: segments.last().unwrap().clone(),
            expected: T::NAME,
            found: resolved_path,
        })
    }

    fn resolve_path_in<'a>(
        &self,
        scope_id: LocalScopeId,
        scope_name: &'a Name,
        segments: &'a [Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        let [qualifiers @ .., name] = segments else { unreachable!() };
        let mut scope = scope_id;
        let mut scope_name = scope_name;
        let mut arc;
        let mut def_map = self;

        for (i, seg) in qualifiers.iter().enumerate() {
            match def_map[scope].children.get(seg) {
                Some(child) => scope = *child,
                None => match def_map[scope].declarations.get(seg) {
                    Some(ScopeItemDef::BlockId(block)) => {
                        if let Some(block_map) = db.block_def_map(*block) {
                            arc = block_map;
                            def_map = &arc;
                            scope = def_map.entry_scope();
                        } else {
                            return Err(PathResolveError::NotFoundIn {
                                name: qualifiers[i + 1].clone(),
                                scope: seg.clone(),
                            });
                        };
                    }
                    Some(ScopeItemDef::BranchId(branch))
                        if qualifiers.get(i + 1) == Some(&kw::potential) =>
                    {
                        let rem = &qualifiers[(i + 1)..];
                        if let [name] = rem {
                            return Ok(ResolvedPath::PotentialAccess {
                                branch: *branch,
                                name: name.clone(),
                            });
                        } else {
                            return Err(PathResolveError::ExpectedNatureAttrIdent {
                                found: rem.to_owned().into_boxed_slice(),
                            });
                        }
                    }
                    Some(ScopeItemDef::BranchId(branch))
                        if qualifiers.get(i + 1) == Some(&kw::flow) =>
                    {
                        let rem = &qualifiers[(i + 1)..];
                        if let [name] = rem {
                            return Ok(ResolvedPath::FlowAccess {
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
                            name: seg.clone(),
                            found: *found,
                        });
                    }
                    None => {
                        return Err(PathResolveError::NotFoundIn {
                            name: seg.clone(),
                            scope: scope_name.clone(),
                        })
                    }
                },
            }
            scope_name = seg;
        }

        match self[scope].declarations.get(name) {
            Some(decl) => Ok(ResolvedPath::ScopeItemDef(*decl)),
            None => {
                Err(PathResolveError::NotFoundIn { name: name.clone(), scope: scope_name.clone() })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPath {
    FlowAccess { branch: BranchId, name: Name },
    PotentialAccess { branch: BranchId, name: Name },
    ScopeItemDef(ScopeItemDef),
}

impl_display! {
    match ResolvedPath{
        ResolvedPath::FlowAccess{..}  => "nature access function";
        ResolvedPath::PotentialAccess{..} => "nature access function";
        ResolvedPath::ScopeItemDef(item) => "{}", item.item_kind();
    }
}

impl_from!(ScopeItemDef for ResolvedPath);
