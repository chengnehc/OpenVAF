//！ Name resolution

use std::ops::{Index, IndexMut};
use stdx::{impl_display, impl_from, impl_from_typed};

use arena::{Arena, Idx};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use syntax::name::{kw, Name};
use syntax::{AstNode, TextRange};

use crate::builtin::{self, BuiltIn, ParamSysFun};
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
    pub fn declare_scope(
        &mut self,
        origin: ScopeOrigin,
        parent: Option<LocalScopeId>,
    ) -> LocalScopeId {
        self.scopes.push_and_get_key(ScopeData {
            origin,
            parent,
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
    Root,                 // Root scope, where natures and disciplines are defined.
    Module(ModuleId),     // scopes created by modules
    Block(BlockId),       // scopes created by named blocks
    Function(FunctionId), // scopes created by analog functions
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
    NatureId(NatureId),
    NatureAttrId(NatureAttrId),
    DisciplineId(DisciplineId),
    ModuleId(ModuleId),
    BlockId(BlockId),
    NodeId(NodeId),
    BranchId(BranchId),
    VarId(VarId),
    ParamId(ParamId),
    AliasParamId(AliasParamId),
    ParamSysFun(ParamSysFun),   // Hierarchical system parameters
    BuiltIn(BuiltIn),           // Builtin functions and sysfuns
    NatureAccess(NatureAccess), // signal access function
    FunctionId(FunctionId),     // user function
    FunctionReturn(FunctionId),
    FunctionArgId(FunctionArgId),
}

impl_from! {
    NatureId, NatureAttrId,
    DisciplineId,
    ModuleId,
    BlockId,
    NodeId,
    BranchId,
    VarId,
    ParamId, AliasParamId, ParamSysFun,
    FunctionId, FunctionArgId, // FunctionReturn
    NatureAccess, BuiltIn     for ScopeItemDef
}

impl ScopeItemDef {
    pub fn text_range(&self, db: &dyn HirDefDB) -> Option<TextRange> {
        use ScopeItemDef::*;

        let res = match self {
            NatureId(nature) => nature.lookup(db).source(db).name()?.syntax().text_range(),
            NatureAttrId(attr) => attr.lookup(db).ast_ptr(db).text_range(),
            NatureAccess(access) => access.0.lookup(db).ast_ptr(db).text_range(),
            DisciplineId(disc) => disc.lookup(db).source(db).name()?.syntax().text_range(),
            ModuleId(module) => module.lookup(db).source(db).name()?.syntax().text_range(),
            BlockId(blk) => blk.lookup(db).source(db).block_scope()?.name()?.syntax().text_range(),
            NodeId(node) => node.lookup(db).ast_ptr(db).text_range(),
            BranchId(branch) => {
                let branch = branch.lookup(db);
                let pos = branch.item_tree(db)[branch.id].name_idx;
                branch.source(db).names().nth(pos)?.syntax().text_range()
            }
            VarId(var) => var.lookup(db).ast_ptr(db).text_range(),
            ParamId(param) => param.lookup(db).ast_ptr(db).text_range(),
            AliasParamId(alias) => alias.lookup(db).ast_ptr(db).text_range(),
            FunctionId(fun) | FunctionReturn(fun) => {
                fun.lookup(db).source(db).name()?.syntax().text_range()
            }
            FunctionArgId(arg) => {
                let arg = arg.lookup(db);
                let fun = arg.fun.lookup(db);
                let pos = fun.item_tree(db)[fun.id].args[arg.id].name_idx;
                arg.source(db).names().nth(pos)?.syntax().text_range()
            }
            BuiltIn(_) | ParamSysFun(_) => return None,
        };

        Some(res)
    }
}

pub trait ScopeItemKind: TryFrom<ScopeItemDef> {
    const NAME: &'static str;
}

macro_rules! scope_item_kinds {
    ($($ty: ident => $name: literal),*) => {
        $(
        impl ScopeItemKind for $ty {
            const NAME: &'static str = $name;
        }
        )*
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
    NatureId => "nature",
    NatureAttrId => "nature attribute",
    NatureAccess => "nature access function",
    DisciplineId => "discipline",
    ModuleId => "module",
    BlockId => "block scope",
    NodeId => "node",
    BranchId => "branch",
    VarId => "variable",
    ParamId => "parameter",
    AliasParamId => "parameter",
    FunctionId => "function",
    FunctionArgId => "function argument",
    BuiltIn => "function",
    ParamSysFun => "hierarchical parameter system function"
}

static BUILTIN_ITEM_DEF: Lazy<IndexMap<Name, ScopeItemDef, ahash::RandomState>> = Lazy::new(|| {
    let mut defs = IndexMap::default();
    builtin::insert_builtin_def(&mut defs);
    defs
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedPath {
    FlowAttr { branch: BranchId, name: Name },
    PotentialAttr { branch: BranchId, name: Name },
    ScopeItemDef(ScopeItemDef),
}
impl_from!(ScopeItemDef for ResolvedPath);
impl_display! {
    match ResolvedPath{
        ResolvedPath::FlowAttr{..}  => "nature attribute";
        ResolvedPath::PotentialAttr{..} => "nature attribute";
        ResolvedPath::ScopeItemDef(item) => "{}", item.item_kind();
    }
}

/// Name resolution algorithms
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

    pub fn resolve_normal_item_path<T: ScopeItemKind>(
        &self,
        scope: LocalScopeId,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<T, PathResolveError> {
        let resolved_path = self.resolve_normal_path(scope, segments, db)?;
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

    // # Scoping Rules
    //
    // ## RootItem: [LRM 6.8]
    // An identifier shall be used to declare only one item within a scope. It is illegal to declare
    // two or more variables which have the same name, or to name a task the same as a variable within
    // the same module, or to give an instance the same name as the name of the net connected to its output.
    //
    // If an identifier is referenced directly (without a hierarchical path) within a named block, it
    // shall be declared within the named block locally or within a module, or within a named block that
    // is higher in the same branch of the name tree containing the named block. If it is declared locally,
    // the local item shall be used; if not, the search shall continue upward until an item by that name is
    // found or until a module boundary is encountered. If the item is a variable, it shall stop at a module
    // boundary; if the item is a named block, it continues to search higher level modules until found.
    //
    // ## FunctionItem: [LRM 4.7.1]
    // * shall only reference locally-defined variables, variables passed as arguments
    // locally-defined parameters and module-level parameters; and
    // * if a locally-defined parameter with the specified name does not exist, then
    // the module-level parameter of the specified name will be used.
    //
    // ## BlockItem: [LRM 5.3.2]
    // The naming of a block allows *local* variables to be declared for that block.
    // * All named block variables are static -- that is, an unique location exists for
    // all variables and leaving or entering the block do not affect the values stored
    // in them.
    // * All identifiers declared within a named sequential block can be accessed
    // outside the scope in which they are declared.
    //      * Named block variables cannot be assigned outside the scope of the block
    //        in which they are declared.
    //      * Parameters declared within a named block have local scope and cannot be
    //        assigned outside the scope.
    pub fn resolve_normal_path(
        &self,
        scope: LocalScopeId,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        let mut cur_scope = scope;
        let mut def_map = self;
        let mut arc;

        let name = segments.first().unwrap();
        let decl = loop {
            // try resolving in current scope
            if let Some(decl) = def_map[cur_scope].declarations.get(name) {
                break *decl;
            }
            // try resolving in parent scopes
            match def_map[cur_scope].parent {
                Some(parent) => cur_scope = parent,
                None => match def_map.src {
                    DefMapSource::Block(block) => {
                        // switch to block def map
                        let parent = block.lookup(db).parent;
                        cur_scope = parent.local_id;
                        arc = parent.def_map(db);
                        def_map = &arc;
                    }
                    DefMapSource::Root | DefMapSource::Function(_) => {
                        // TODO when dealing with function def map, give hint if a builtin decl
                        // is found in root def map. So far, these two are identical.
                        let Some(builtin) = BUILTIN_ITEM_DEF.get(name) else {
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

        cur_scope = match decl {
            ScopeItemDef::ModuleId(module) => module.lookup(db).scope.local_id,
            ScopeItemDef::BlockId(block) => {
                let Some(block_map) = db.block_def_map(block) else {
                    let name = segments[1].clone();
                    let scope = segments[0].clone();
                    return Err(PathResolveError::NotFoundIn { name, scope });
                };
                arc = block_map;
                def_map = &arc;
                def_map.entry_scope()
            }
            _ => return Err(PathResolveError::ExpectedScope { name: name.clone(), found: decl }),
        };

        def_map.resolve_path(cur_scope, name, &segments[1..], db)
    }

    pub fn resolve_root_item_path<T: ScopeItemKind>(
        &self,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<T, PathResolveError> {
        let resolved_path = self.resolve_root_path(segments, db)?;
        let res: Result<ScopeItemDef, _> = resolved_path.clone().try_into();
        if let Ok(def) = res {
            if let Ok(item) = def.try_into() {
                return Ok(item);
            }
        }
        Err(PathResolveError::ExpectedItemKind {
            name: segments.last().unwrap().clone(),
            expected: T::NAME,
            found: resolved_path,
        })
    }

    /// Resolve a path with `$root` prefix. [LRM 6.2.1]
    pub fn resolve_root_path(
        &self,
        segments: &[Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        // JW: paths in named block should be resolved in the root def map
        // paths in analog function should be resolved in the function's own def map
        // even if $root is given
        debug_assert!(matches!(self.src, DefMapSource::Root | DefMapSource::Function(_)));
        self.resolve_path(self.root_scope(), &kw::root, segments, db)
    }

    fn resolve_path<'a>(
        &self,
        scope_id: LocalScopeId,
        scope_name: &'a Name,
        segments: &'a [Name],
        db: &dyn HirDefDB,
    ) -> Result<ResolvedPath, PathResolveError> {
        let [segments @ .., name] = segments else { unreachable!() };
        let mut scope = scope_id;
        let mut scope_name = scope_name;
        let mut arc;
        let mut def_map = self;

        for (i, seg) in segments.iter().enumerate() {
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
                                name: segments[i + 1].clone(),
                                scope: seg.clone(),
                            });
                        };
                    }
                    Some(ScopeItemDef::BranchId(branch))
                        if segments.get(i + 1) == Some(&kw::potential) =>
                    {
                        let rem = &segments[(i + 1)..];
                        if let [name] = rem {
                            return Ok(ResolvedPath::PotentialAttr {
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
                        if segments.get(i + 1) == Some(&kw::flow) =>
                    {
                        let rem = &segments[(i + 1)..];
                        if let [name] = rem {
                            return Ok(ResolvedPath::FlowAttr {
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
