use super::*;

use std::sync::LazyLock;
use stdx::impl_from_typed;

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum Scope {
    /// Recursive scopes that can have children scopes
    Rec(RecScope),
    /// Flat scopes that have no children scopes
    Flat(FlatScope),
}
impl_from_typed! { Rec(RecScope), Flat(FlatScope) for Scope }

impl Scope {
    pub fn origin(&self) -> ScopeOrigin {
        match self {
            Scope::Rec(scope) => scope.origin,
            Scope::Flat(scope) => scope.origin,
        }
    }
    pub fn parent(&self) -> Option<LocalScopeId> {
        match self {
            Scope::Rec(scope) => scope.parent,
            Scope::Flat(scope) => scope.parent,
        }
    }
    pub fn children(&self) -> Option<&IndexMap<Name, LocalScopeId, ahash::RandomState>> {
        if let Scope::Rec(scope) = self {
            Some(&scope.children)
        } else {
            None
        }
    }
    pub fn children_mut(
        &mut self,
    ) -> Option<&mut IndexMap<Name, LocalScopeId, ahash::RandomState>> {
        if let Scope::Rec(scope) = self {
            Some(&mut scope.children)
        } else {
            None
        }
    }
    pub fn decls(&self) -> &IndexMap<Name, ScopeItem, ahash::RandomState> {
        match self {
            Scope::Rec(scope) => &scope.declarations,
            Scope::Flat(scope) => &scope.declarations,
        }
    }
    pub fn decls_mut(&mut self) -> &mut IndexMap<Name, ScopeItem, ahash::RandomState> {
        match self {
            Scope::Rec(scope) => &mut scope.declarations,
            Scope::Flat(scope) => &mut scope.declarations,
        }
    }
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct RecScope {
    /// where does this scope originate from?
    pub origin: ScopeOrigin,
    /// parent scope, `None` for root scope
    pub parent: Option<LocalScopeId>,
    /// children scopes defined within this scope
    pub children: IndexMap<Name, LocalScopeId, ahash::RandomState>,
    /// item declarations in this scope
    pub declarations: IndexMap<Name, ScopeItem, ahash::RandomState>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct FlatScope {
    pub origin: ScopeOrigin,
    pub parent: Option<LocalScopeId>,
    pub declarations: IndexMap<Name, ScopeItem, ahash::RandomState>,
}

/// Where does the scope originate from?
#[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
pub enum ScopeOrigin {
    Root,
    Module(ModuleId),
    Block(BlockId),
    Function(FunctionId),
}
impl_from_typed! { Module(ModuleId), Block(BlockId), Function(FunctionId) for ScopeOrigin }

use syntax::{AstNode, TextRange};

use crate::builtin::{self, BuiltIn, ParamSysFun};
use crate::{
    AliasParamId, BlockId, BranchId, DisciplineAttrId, DisciplineId, FunctionArgId, FunctionId,
    ModuleId, NatureAccess, NatureAttrId, NatureId, NodeId, ParamId, VarId,
};

/// Verilog-A language builtin item definitions
pub static BUILTIN_ITEM_DEF: LazyLock<IndexMap<Name, ScopeItem, ahash::RandomState>> =
    LazyLock::new(|| {
        let mut defs = IndexMap::default();
        builtin::insert_builtin_def(&mut defs);
        defs
    });

/// Item definitions within a scope that are made by user.
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
    pub(crate) fn text_range(self, db: &dyn HirDefDB) -> Option<TextRange> {
        let res = match self {
            // items with large span
            Self::NatureId(nature) => nature.lookup(db).source(db).name()?.syntax().text_range(),
            Self::DisciplineId(disc) => disc.lookup(db).source(db).name()?.syntax().text_range(),
            Self::ModuleId(module) => module.lookup(db).source(db).name()?.syntax().text_range(),
            Self::BlockId(blk) => {
                blk.lookup(db).source(db).block_scope()?.name()?.syntax().text_range()
            }
            Self::FunctionId(fun) | Self::FunctionReturn(fun) => {
                fun.lookup(db).source(db).name()?.syntax().text_range()
            }

            // items with small span
            Self::NatureAttrId(attr) => attr.lookup(db).ast_ptr(db).text_range(),
            Self::NatureAccess(access) => access.0.lookup(db).ast_ptr(db).text_range(),
            Self::NodeId(node) => node.lookup(db).ast_ptr(db).text_range(),
            Self::VarId(var) => var.lookup(db).ast_ptr(db).text_range(),
            Self::ParamId(param) => param.lookup(db).ast_ptr(db).text_range(),
            Self::AliasParamId(alias) => alias.lookup(db).ast_ptr(db).text_range(),

            // items with argument list
            Self::BranchId(branch) => {
                let branch = branch.lookup(db);
                let pos = branch.item_tree(db)[branch.id].name_idx;
                branch.source(db).names().nth(pos)?.syntax().text_range()
            }
            Self::FunctionArgId(arg) => {
                let arg = arg.lookup(db);
                let fun = arg.fun.lookup(db);
                let pos = fun.item_tree(db)[fun.id].args[arg.id].name_idx;
                arg.source(db).names().nth(pos)?.syntax().text_range()
            }

            Self::BuiltIn(_) | Self::ParamSysFun(_) => return None,
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

/// Some items can have a body that contains statements and expressions
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ItemWithBodyId {
    NatureAttrId(NatureAttrId),
    DisciplineAttrId(DisciplineAttrId),
    ModuleId { initial: bool, id: ModuleId },
    VarId(VarId),
    ParamId(ParamId),
    FunctionId(FunctionId),
}
impl_from!(NatureAttrId, DisciplineAttrId, VarId, ParamId, FunctionId for ItemWithBodyId);

impl TryFrom<ScopeItem> for ItemWithBodyId {
    type Error = ();
    fn try_from(src: ScopeItem) -> Result<ItemWithBodyId, ()> {
        let res = match src {
            ScopeItem::NatureAttrId(attr) => attr.into(),
            ScopeItem::VarId(var) => var.into(),
            ScopeItem::ParamId(param) => param.into(),
            ScopeItem::FunctionId(fun) => fun.into(),
            _ => return Err(()),
        };

        Ok(res)
    }
}

impl ItemWithBodyId {
    pub fn file(self, db: &dyn HirDefDB) -> FileId {
        match self {
            ItemWithBodyId::NatureAttrId(id) => id.lookup(db).nature.lookup(db).root_file,
            ItemWithBodyId::DisciplineAttrId(id) => id.lookup(db).discipline.lookup(db).root_file,
            ItemWithBodyId::ModuleId { id, .. } => id.lookup(db).scope.root_file,
            ItemWithBodyId::VarId(id) => id.lookup(db).scope.root_file,
            ItemWithBodyId::ParamId(id) => id.lookup(db).scope.root_file,
            ItemWithBodyId::FunctionId(id) => id.lookup(db).scope.root_file,
        }
    }
}
