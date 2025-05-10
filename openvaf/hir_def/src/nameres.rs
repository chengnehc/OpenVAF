//！ Name resolution

use std::ops::{Index, IndexMut};
use stdx::{impl_from, impl_from_typed};

use arena::{Arena, Idx};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use syntax::name::Name;
use syntax::{AstNode, TextRange};

use crate::builtin::{self, BuiltIn, ParamSysFun};
use crate::db::HirDefDB;
use crate::{
    AliasParamId, BlockId, BranchId, DisciplineId, FunctionArgId, FunctionId, Lookup, ModuleId,
    NatureAccess, NatureAttrId, NatureId, NodeId, ParamId, VarId,
};

mod collect;
mod diagnostics;
mod path;
mod pretty;

use diagnostics::DefDiagnostic;
pub use diagnostics::{DefDiagnosticWrapped, PathResolveError};
pub use path::ResolvedPath;

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
    #[inline(always)]
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
    Root,                 // top-level items: natures, disciplines and modules
    Block(BlockId),       // named blocks
    Function(FunctionId), // analog functions
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
    NatureId, NatureAttrId, NatureAccess,
    DisciplineId,
    ModuleId,
    BlockId,
    NodeId,
    BranchId,
    VarId,
    ParamId,
    AliasParamId,
    FunctionId, FunctionArgId, // FunctionReturn
    ParamSysFun, BuiltIn     for ScopeItemDef
}

impl ScopeItemDef {
    fn text_range(self, db: &dyn HirDefDB) -> Option<TextRange> {
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
