//！ Name resolution

use std::ops::{Index, IndexMut};
use stdx::{impl_from, impl_from_typed};

use arena::{Arena, Idx};
use indexmap::IndexMap;
use once_cell::sync::Lazy;
use syntax::{AstNode, Name, TextRange};

use crate::builtin::{self, BuiltIn, ParamSysFun};
use crate::db::HirDefDB;
use crate::{
    AliasParamId, BlockId, BranchId, DisciplineId, FunctionArgId, FunctionId, Lookup, ModuleId,
    NatureAccess, NatureAttrId, NatureId, NodeId, ParamId, VarId,
};

mod collect;
mod diagnostics;
mod pathres;
mod pretty;

use diagnostics::DefDiagnostic;
pub use diagnostics::{DefDiagnosticWrapped, PathResolveError};
pub use pathres::ResolvedPath;

/// Definition map. It contains the results of name resolution.
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
    pub declarations: IndexMap<Name, ScopeItem, ahash::RandomState>,
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
    Block(BlockId),
    Function(FunctionId),
}

impl_from_typed! {
    Module(ModuleId),
    Block(BlockId),
    Function(FunctionId) for ScopeOrigin
}

/// Items that can be defined within a scope.
#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub enum ScopeItem {
    NatureId(NatureId),
    NatureAttrId(NatureAttrId),
    NatureAccess(NatureAccess),
    DisciplineId(DisciplineId),
    ModuleId(ModuleId),
    BlockId(BlockId),
    NodeId(NodeId),
    BranchId(BranchId),
    VarId(VarId),
    ParamId(ParamId),
    AliasParamId(AliasParamId),
    ParamSysFun(ParamSysFun),
    BuiltIn(BuiltIn),
    FunctionId(FunctionId),
    FunctionArgId(FunctionArgId),
    FunctionReturn(FunctionId),
}

impl_from! {
    NatureId,
    NatureAttrId,
    NatureAccess,
    DisciplineId,
    ModuleId,
    BlockId,
    NodeId,
    BranchId,
    VarId,
    ParamId,
    AliasParamId,
    ParamSysFun,
    BuiltIn,
    FunctionId,
    FunctionArgId   for ScopeItem
}

impl ScopeItem {
    fn text_range(self, db: &dyn HirDefDB) -> Option<TextRange> {
        use ScopeItem::*;

        let res = match self {
            // items with large span
            NatureId(nature) => nature.lookup(db).source(db).name()?.syntax().text_range(),
            DisciplineId(disc) => disc.lookup(db).source(db).name()?.syntax().text_range(),
            ModuleId(module) => module.lookup(db).source(db).name()?.syntax().text_range(),
            BlockId(blk) => blk.lookup(db).source(db).block_scope()?.name()?.syntax().text_range(),
            FunctionId(fun) | FunctionReturn(fun) => {
                fun.lookup(db).source(db).name()?.syntax().text_range()
            }

            // items with small span
            NatureAttrId(attr) => attr.lookup(db).ast_ptr(db).text_range(),
            NatureAccess(access) => access.0.lookup(db).ast_ptr(db).text_range(),
            NodeId(node) => node.lookup(db).ast_ptr(db).text_range(),
            VarId(var) => var.lookup(db).ast_ptr(db).text_range(),
            ParamId(param) => param.lookup(db).ast_ptr(db).text_range(),
            AliasParamId(alias) => alias.lookup(db).ast_ptr(db).text_range(),

            // items with argument list
            BranchId(branch) => {
                let branch = branch.lookup(db);
                let pos = branch.item_tree(db)[branch.id].name_idx;
                branch.source(db).names().nth(pos)?.syntax().text_range()
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

pub trait ScopeItemKind: TryFrom<ScopeItem> {
    const NAME: &'static str;
}

macro_rules! scope_item_kinds {
    ($($ty: ident => $name: literal),*) => {
        $(
        impl ScopeItemKind for $ty {
            const NAME: &'static str = $name;
        }
        )*
        impl ScopeItem {
            pub const fn item_kind(&self) -> &'static str {
                match self {
                    $(ScopeItem::$ty(_) => $ty::NAME,)*
                    ScopeItem::FunctionReturn(_) => VarId::NAME
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

static BUILTIN_ITEM_DEF: Lazy<IndexMap<Name, ScopeItem, ahash::RandomState>> = Lazy::new(|| {
    let mut defs = IndexMap::default();
    builtin::insert_builtin_def(&mut defs);
    defs
});
