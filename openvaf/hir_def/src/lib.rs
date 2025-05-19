//! See Also:
//! - https://docs.rs/ra_ap_hir_def/latest/ra_ap_hir_def/index.html

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use stdx::impl_debug_display;

use arena::Idx;
use basedb::{AstId, ErasedAstId, FileId};
use syntax::{ast, AstNode, AstPtr, Name, SyntaxNodePtr};

pub mod body;
pub mod db;
pub mod nameres;

mod builtin;
mod data;
mod item_tree;
mod path;
mod types;

pub use body::{Expr, Stmt};
pub use builtin::{BuiltIn, ParamSysFun};
pub use item_tree::{
    AliasParam, Branch, BranchKind, Discipline, DisciplineAttr, Function, ItemTree, ItemTreeId,
    ItemTreeNode, Module, Nature, NatureAttr, NatureRef, NatureRefKind, NodeTypeDecl, Param, Var,
};
pub use path::Path;
pub use types::Type;

use db::HirDefDB;
use nameres::{DefMap, ScopeId};

#[derive(Debug)]
pub struct ItemLoc<N: ItemTreeNode> {
    pub scope: ScopeId,
    pub id: ItemTreeId<N>,
}

impl<N: ItemTreeNode> ItemLoc<N> {
    #[inline]
    pub fn item_tree(&self, db: &dyn HirDefDB) -> Arc<ItemTree> {
        db.item_tree(self.scope.root_file)
    }
    #[inline]
    pub fn def_map(&self, db: &dyn HirDefDB) -> Arc<DefMap> {
        self.scope.def_map(db)
    }
    #[inline]
    pub fn name(&self, db: &dyn HirDefDB) -> Name {
        N::lookup(&self.item_tree(db), self.id).name().clone()
    }
    #[inline]
    pub fn ast_id(&self, db: &dyn HirDefDB) -> AstId<N::Source> {
        N::lookup(&self.item_tree(db), self.id).ast_id()
    }
    pub fn ast_ptr(&self, db: &dyn HirDefDB) -> AstPtr<N::Source> {
        let ast_id = self.ast_id(db);
        db.ast_id_map(self.scope.root_file).get(ast_id)
    }
    pub fn source(&self, db: &dyn HirDefDB) -> N::Source {
        let root = db.parse(self.scope.root_file).tree();
        self.ast_ptr(db).to_node(root.syntax())
    }
}

impl<N: ItemTreeNode> Clone for ItemLoc<N> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<N: ItemTreeNode> Copy for ItemLoc<N> {}

impl<N: ItemTreeNode> PartialEq for ItemLoc<N> {
    fn eq(&self, other: &Self) -> bool {
        self.scope == other.scope && self.id == other.id
    }
}
impl<N: ItemTreeNode> Eq for ItemLoc<N> {}

impl<N: ItemTreeNode> Hash for ItemLoc<N> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scope.hash(state);
        self.id.hash(state);
    }
}

// Traits to intern item data into and lookup data from salsa database.
//
// `...Id` are new-typed wrappers around salsa::InternId
// `...Loc` contains item data: scope and ID, the ID could be:
//  - ID of an item in the `ItemTree`
//  - ID local to the item tree node's arena
//  - ID of the `AstIdMap`
pub trait Intern {
    type Id;
    fn intern(self, db: &dyn HirDefDB) -> Self::Id;
}
pub trait Lookup {
    type Data;
    fn lookup(&self, db: &dyn HirDefDB) -> Self::Data;
}

macro_rules! impl_intern {
    ($id:ident, $loc:ident, $intern:ident, $lookup:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub struct $id(salsa::InternId);

        impl_intern_key!($id);
        impl_intern_lookup!($id, $loc, $intern, $lookup);
    };
}
// Implement `InternKey` trait for `...Id`
macro_rules! impl_intern_key {
    ($id:ident) => {
        impl salsa::InternKey for $id {
            fn from_intern_id(raw: salsa::InternId) -> Self {
                $id(raw)
            }
            fn as_intern_id(&self) -> salsa::InternId {
                self.0
            }
        }
    };
}
// Implement `Intern` trait for item location and `Lookup` trait for item ID
macro_rules! impl_intern_lookup {
    ($id:ident, $loc:ident, $intern:ident, $lookup:ident) => {
        impl Intern for $loc {
            type Id = $id;
            fn intern(self, db: &dyn HirDefDB) -> $id {
                db.$intern(self)
            }
        }
        impl Lookup for $id {
            type Data = $loc;
            fn lookup(&self, db: &dyn HirDefDB) -> $loc {
                db.$lookup(*self)
            }
        }
    };
}

/* Nature, Discipline */

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NatureLoc {
    pub root_file: FileId,
    pub id: ItemTreeId<Nature>,
}
impl_intern!(NatureId, NatureLoc, intern_nature, lookup_intern_nature);
impl NatureLoc {
    pub fn item_tree(self, db: &dyn HirDefDB) -> Arc<ItemTree> {
        db.item_tree(self.root_file)
    }
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::NatureDecl> {
        Nature::lookup(&self.item_tree(db), self.id).ast_id()
    }
    pub fn source(self, db: &dyn HirDefDB) -> ast::NatureDecl {
        let ast_id = self.ast_id(db);
        db.ast_id_map(self.root_file).get(ast_id).to_node(db.parse(self.root_file).tree().syntax())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisciplineLoc {
    pub root_file: FileId,
    pub id: ItemTreeId<Discipline>,
}
impl_intern!(DisciplineId, DisciplineLoc, intern_discipline, lookup_intern_discipline);
impl DisciplineLoc {
    pub fn item_tree(self, db: &dyn HirDefDB) -> Arc<ItemTree> {
        db.item_tree(self.root_file)
    }
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::DisciplineDecl> {
        Discipline::lookup(&self.item_tree(db), self.id).ast_id()
    }
    pub fn source(self, db: &dyn HirDefDB) -> ast::DisciplineDecl {
        let ast_id = self.ast_id(db);
        db.ast_id_map(self.root_file).get(ast_id).to_node(db.parse(self.root_file).tree().syntax())
    }
}

/* Nature and discipline attribute */

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct NatureAttrLoc {
    pub nature: NatureId,
    pub id: LocalNatureAttrId,
}
pub type LocalNatureAttrId = Idx<data::NatureAttrData>;
impl_intern!(NatureAttrId, NatureAttrLoc, intern_nature_attr, lookup_intern_nature_attr);
impl NatureAttrLoc {
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::NatureAttr> {
        let nature = self.nature.lookup(db);
        let tree = nature.item_tree(db);
        let attrs = &tree[nature.id].attrs;
        let attr_id: ItemTreeId<item_tree::NatureAttr> =
            (u32::from(attrs.start()) + u32::from(self.id)).into();
        tree[attr_id].ast_id()
    }
    pub fn ast_ptr(self, db: &dyn HirDefDB) -> AstPtr<ast::NatureAttr> {
        let file = self.nature.lookup(db).root_file;
        db.ast_id_map(file).get(self.ast_id(db))
    }
}

/// Nature access function is a special kind of nature attribute.
#[derive(Debug, Hash, Clone, Copy, PartialEq, Eq)]
pub struct NatureAccess(pub NatureAttrId);

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct DisciplineAttrLoc {
    pub discipline: DisciplineId,
    pub id: LocalDisciplineAttrId,
}
pub type LocalDisciplineAttrId = Idx<data::DisciplineAttrData>;
impl_intern!(
    DisciplineAttrId,
    DisciplineAttrLoc,
    intern_discipline_attr,
    lookup_intern_discipline_attr
);

/* Item tree nodes */

pub type ModuleLoc = ItemLoc<Module>;
impl_intern!(ModuleId, ModuleLoc, intern_module, lookup_intern_module);

pub type BranchLoc = ItemLoc<Branch>;
impl_intern!(BranchId, BranchLoc, intern_branch, lookup_intern_branch);

pub type VarLoc = ItemLoc<Var>;
impl_intern!(VarId, VarLoc, intern_var, lookup_intern_var);

pub type ParamLoc = ItemLoc<Param>;
impl_intern!(ParamId, ParamLoc, intern_param, lookup_intern_param);

pub type AliasParamLoc = ItemLoc<AliasParam>;
impl_intern!(AliasParamId, AliasParamLoc, intern_aliasparam, lookup_intern_aliasparam);

pub type FunctionLoc = ItemLoc<Function>;
impl_intern!(FunctionId, FunctionLoc, intern_function, lookup_intern_function);

/* Node (port and net) */

#[derive(Clone, Copy, PartialEq, PartialOrd, Eq, Hash)]
pub struct NodeId(salsa::InternId);
impl_debug_display!(match NodeId{ NodeId(id) => "node{id:?}";});

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct NodeLoc {
    pub module: ModuleId,
    pub id: LocalNodeId,
}
pub type LocalNodeId = Idx<item_tree::Node>;
impl_intern_key!(NodeId);
impl_intern_lookup!(NodeId, NodeLoc, intern_node, lookup_intern_node);
impl NodeLoc {
    pub fn ast_ptr(self, db: &dyn HirDefDB) -> SyntaxNodePtr {
        let module = self.module.lookup(db);
        let ast_id = module.item_tree(db)[module.id].nodes[self.id].ast_id;
        let file = module.scope.root_file;
        db.ast_id_map(file).get_erased(ast_id)
    }
    pub fn ast_id(self, db: &dyn HirDefDB) -> Option<ErasedAstId> {
        let module = self.module.lookup(db);
        let tree = module.item_tree(db);
        let node = &tree[module.id].nodes[self.id];
        node.decls.iter().find(|decl| decl.discipline(&tree).is_some()).map(|it| it.ast_id(&tree))
    }
}

/* Function argument */

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct FunctionArgLoc {
    pub fun: FunctionId,
    pub id: LocalFunctionArgId,
}
pub type LocalFunctionArgId = Idx<item_tree::FunctionArg>;
impl_intern!(FunctionArgId, FunctionArgLoc, intern_function_arg, lookup_intern_function_arg);
impl FunctionArgLoc {
    pub fn name(self, db: &dyn HirDefDB) -> Name {
        let fun = self.fun.lookup(db);
        fun.item_tree(db)[fun.id].args[self.id].name.clone()
    }
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::FunctionArg> {
        let fun = self.fun.lookup(db);
        fun.item_tree(db)[fun.id].args[self.id].ast_id
    }
    pub fn ast_ptr(self, db: &dyn HirDefDB) -> AstPtr<ast::FunctionArg> {
        db.ast_id_map(self.fun.lookup(db).scope.root_file).get(self.ast_id(db))
    }
    pub fn source(self, db: &dyn HirDefDB) -> ast::FunctionArg {
        let file = self.fun.lookup(db).scope.root_file;
        let ptr = db.ast_id_map(file).get(self.ast_id(db));
        ptr.to_node(db.parse(file).tree().syntax())
    }
}

/* Block */

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockLoc {
    parent: ScopeId,
    ast_id: AstId<ast::BlockStmt>,
}
impl_intern!(BlockId, BlockLoc, intern_block, lookup_intern_block);
impl BlockLoc {
    pub fn name(self, db: &dyn HirDefDB) -> Name {
        let tree = db.item_tree(self.parent.root_file);
        tree.blocks[&self.ast_id].name.clone().expect("Only named blocks shall be interned")
    }
    pub fn source(self, db: &dyn HirDefDB) -> ast::BlockStmt {
        let file = self.parent.root_file;
        let ptr = db.ast_id_map(file).get(self.ast_id);
        ptr.to_node(db.parse(file).tree().syntax())
    }
}
