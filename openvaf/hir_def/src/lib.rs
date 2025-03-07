//! See Also:
//!
//! https://docs.rs/ra_ap_hir_def/latest/ra_ap_hir_def/index.html

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use stdx::{impl_debug_display, impl_from};

use arena::Idx;
use basedb::{AstId, ErasedAstId, FileId};
use syntax::name::Name;
use syntax::{ast, AstNode, AstPtr, SyntaxNodePtr};

pub mod body;
pub mod db;
pub mod expr;
pub mod nameres;

mod builtin;
mod data;
mod item_tree;
mod path;
mod types;

pub use crate::builtin::{BuiltIn, ParamSysFun};
pub use crate::data::FunctionArgData;
pub use crate::expr::{Case, Expr, ExprId, Literal, Stmt, StmtId};
pub use crate::item_tree::{
    AliasParam, Branch, BranchKind, Discipline, DisciplineAttr, Function, ItemTree, ItemTreeId,
    ItemTreeNode, Module, Nature, NatureAttr, NatureRef, NatureRefKind, NodeTypeDecl, Param, Var,
};
pub use crate::path::Path;
pub use crate::types::Type;

use crate::db::HirDefDB;
use crate::nameres::{
    DefMap, DefMapSource, PathResolveError, ResolvedPath, ScopeItemDef, ScopeItemKind,
};

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct Scope {
    pub root_file: FileId,
    /// Which kind of `DefMap` does this scope correspond to?
    pub src: DefMapSource,
    /// The scope's ID **local** to the `DefMap`
    pub local_id: nameres::LocalScopeId,
}

impl Scope {
    pub fn new(root_file: FileId, src: DefMapSource, local_id: nameres::LocalScopeId) -> Self {
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
            _ => self.def_map(db).resolve_normal_path_in(self.local_id, &path.segments, db),
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
            _ => self.def_map(db).resolve_normal_item_path_in(self.local_id, &path.segments, db),
        }
    }
}

#[derive(Debug)]
pub struct ItemLoc<N: ItemTreeNode> {
    pub scope: Scope,
    pub id: ItemTreeId<N>,
}

impl<N: ItemTreeNode> ItemLoc<N> {
    pub fn item_tree(&self, db: &dyn HirDefDB) -> Arc<ItemTree> {
        db.item_tree(self.scope.root_file)
    }

    pub fn ast_id(&self, db: &dyn HirDefDB) -> AstId<N::Source> {
        N::lookup(&self.item_tree(db), self.id).ast_id()
    }

    pub fn ast_ptr(&self, db: &dyn HirDefDB) -> AstPtr<N::Source> {
        let ast_id = self.ast_id(db);
        db.ast_id_map(self.scope.root_file).get(ast_id)
    }

    pub fn source(&self, db: &dyn HirDefDB) -> N::Source {
        self.ast_ptr(db).to_node(db.parse(self.scope.root_file).tree().syntax())
    }

    pub fn name(&self, db: &dyn HirDefDB) -> Name {
        N::lookup(&self.item_tree(db), self.id).name().clone()
    }

    pub fn def_map(&self, db: &dyn HirDefDB) -> Arc<DefMap> {
        self.scope.def_map(db)
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

// Traits for interning and looking up items in DB.
pub trait Intern {
    type Id;
    fn intern(self, db: &dyn HirDefDB) -> Self::Id;
}
pub trait Lookup {
    type Data;
    fn lookup(&self, db: &dyn HirDefDB) -> Self::Data;
}

// Macros for implementing `Intern` and `Lookup` trait for HIR items.
macro_rules! impl_intern {
    ($id:ident, $loc:ident, $intern:ident, $lookup:ident) => {
        impl_intern_key!($id);
        impl_intern_lookup!($id, $loc, $intern, $lookup);
    };
}
macro_rules! impl_intern_key {
    ($name:ident) => {
        impl salsa::InternKey for $name {
            fn from_intern_id(v: salsa::InternId) -> Self {
                $name(v)
            }
            fn as_intern_id(&self) -> salsa::InternId {
                self.0
            }
        }
    };
}
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

/* The intern and lookup impls for items in the salsa database */

// - `...Id` are new-typed wrappers around salsa::InternId.
// - `...Loc` contains information about the item's scope and item tree id,
//   it can both be a instantiation of ItemLoc<T> or manually defined.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisciplineId(salsa::InternId);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisciplineLoc {
    pub root_file: FileId, // disciplines have global scope
    pub id: ItemTreeId<Discipline>,
}
impl DisciplineLoc {
    pub fn item_tree(self, db: &dyn HirDefDB) -> Arc<ItemTree> {
        db.item_tree(self.root_file)
    }
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::DisciplineDecl> {
        // self.item_tree(db)[self.id].ast_id
        // JW: this should be more idiomatic
        Discipline::lookup(&self.item_tree(db), self.id).ast_id()
    }
    pub fn source(self, db: &dyn HirDefDB) -> ast::DisciplineDecl {
        let ast_id = self.ast_id(db);
        db.ast_id_map(self.root_file).get(ast_id).to_node(db.parse(self.root_file).tree().syntax())
    }
}
impl_intern!(DisciplineId, DisciplineLoc, intern_discipline, lookup_intern_discipline);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NatureId(salsa::InternId);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NatureLoc {
    pub root_file: FileId, // natures have global scope
    pub id: ItemTreeId<Nature>,
}
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
impl_intern!(NatureId, NatureLoc, intern_nature, lookup_intern_nature);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisciplineAttrId(salsa::InternId);
pub type LocalDisciplineAttrId = Idx<data::DisciplineAttrData>;
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct DisciplineAttrLoc {
    pub discipline: DisciplineId,
    pub id: LocalDisciplineAttrId,
}
impl_intern!(
    DisciplineAttrId,
    DisciplineAttrLoc,
    intern_discipline_attr,
    lookup_intern_discipline_attr
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NatureAttrId(salsa::InternId);
pub type LocalNatureAttrId = Idx<data::NatureAttrData>;
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct NatureAttrLoc {
    pub nature: NatureId,
    pub id: LocalNatureAttrId,
}
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
impl_intern!(NatureAttrId, NatureAttrLoc, intern_nature_attr, lookup_intern_nature_attr);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(salsa::InternId);
pub type ModuleLoc = ItemLoc<Module>;
impl_intern!(ModuleId, ModuleLoc, intern_module, lookup_intern_module);

// We only intern nodes, not ports or nets (too detailed).
#[derive(Clone, Copy, PartialEq, PartialOrd, Eq, Hash)]
pub struct NodeId(salsa::InternId);
pub type LocalNodeId = Idx<item_tree::Node>;
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct NodeLoc {
    pub module: ModuleId,
    pub id: LocalNodeId,
}
impl NodeLoc {
    pub fn ast_id(self, db: &dyn HirDefDB) -> ErasedAstId {
        let module = self.module.lookup(db);
        module.item_tree(db)[module.id].nodes[self.id].ast_id
    }
    pub fn ast_ptr(self, db: &dyn HirDefDB) -> SyntaxNodePtr {
        let module = self.module.lookup(db);
        let ast_id = module.item_tree(db)[module.id].nodes[self.id].ast_id;
        let file = module.scope.root_file;
        db.ast_id_map(file).get_erased(ast_id)
    }
    pub fn discipline_ast_id(self, db: &dyn HirDefDB) -> Option<ErasedAstId> {
        let module = self.module.lookup(db);
        let tree = module.item_tree(db);
        let node = &tree[module.id].nodes[self.id];
        node.decls.iter().find(|decl| decl.discipline(&tree).is_some()).map(|it| it.ast_id(&tree))
    }
}
impl_debug_display!(match NodeId{ NodeId(id) => "node{:?}", id;});
impl_intern!(NodeId, NodeLoc, intern_node, lookup_intern_node);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BranchId(salsa::InternId);
pub type BranchLoc = ItemLoc<Branch>;
impl_intern!(BranchId, BranchLoc, intern_branch, lookup_intern_branch);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VarId(salsa::InternId);
pub type VarLoc = ItemLoc<Var>;
impl_intern!(VarId, VarLoc, intern_var, lookup_intern_var);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ParamId(salsa::InternId);
pub type ParamLoc = ItemLoc<Param>;
impl_intern!(ParamId, ParamLoc, intern_param, lookup_intern_param);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AliasParamId(salsa::InternId);
pub type AliasParamLoc = ItemLoc<AliasParam>;
impl_intern!(AliasParamId, AliasParamLoc, intern_alias_param, lookup_intern_alias_param);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(salsa::InternId);
pub type FunctionLoc = ItemLoc<Function>;
impl_intern!(FunctionId, FunctionLoc, intern_function, lookup_intern_function);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionArgId(salsa::InternId);
pub type LocalFunctionArgId = Idx<item_tree::FunctionArg>;
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct FunctionArgLoc {
    pub fun: FunctionId,
    pub id: LocalFunctionArgId,
}
impl FunctionArgLoc {
    pub fn name(self, db: &dyn HirDefDB) -> Name {
        let fun = self.fun.lookup(db);
        fun.item_tree(db)[fun.id].args[self.id].name.clone()
    }
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::FunctionArg> {
        let fun = self.fun.lookup(db);
        fun.item_tree(db)[fun.id].args[self.id].ast_ids[0]
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
impl_intern!(FunctionArgId, FunctionArgLoc, intern_function_arg, lookup_intern_function_arg);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(salsa::InternId);
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockLoc {
    parent: Scope,
    ast_id: AstId<ast::BlockStmt>,
}
impl BlockLoc {
    pub fn name(self, db: &dyn HirDefDB) -> Name {
        let tree = db.item_tree(self.parent.root_file);
        tree[self.ast_id].name.clone().expect("BlockLocs are only created for named Blocks")
    }
    pub fn source(self, db: &dyn HirDefDB) -> ast::BlockStmt {
        let file = self.parent.root_file;
        let ptr = db.ast_id_map(file).get(self.ast_id);
        ptr.to_node(db.parse(file).tree().syntax())
    }
}
impl_intern!(BlockId, BlockLoc, intern_block, lookup_intern_block);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefWithBodyId {
    ModuleId { initial: bool, id: ModuleId },
    DisciplineAttrId(DisciplineAttrId),
    NatureAttrId(NatureAttrId),
    VarId(VarId),
    ParamId(ParamId),
    FunctionId(FunctionId),
}
impl_from!(ParamId, FunctionId, VarId, NatureAttrId, DisciplineAttrId for DefWithBodyId);

impl TryFrom<ScopeItemDef> for DefWithBodyId {
    type Error = ();
    fn try_from(src: ScopeItemDef) -> Result<DefWithBodyId, ()> {
        let res = match src {
            ScopeItemDef::NatureAttrId(attr) => attr.into(),
            ScopeItemDef::VarId(var) => var.into(),
            ScopeItemDef::ParamId(param) => param.into(),
            ScopeItemDef::FunctionId(fun) => fun.into(),
            _ => return Err(()),
        };
        Ok(res)
    }
}

impl DefWithBodyId {
    pub fn file(self, db: &dyn HirDefDB) -> FileId {
        match self {
            DefWithBodyId::ModuleId { id, .. } => id.lookup(db).scope.root_file,
            DefWithBodyId::DisciplineAttrId(id) => id.lookup(db).discipline.lookup(db).root_file,
            DefWithBodyId::NatureAttrId(id) => id.lookup(db).nature.lookup(db).root_file,
            DefWithBodyId::VarId(id) => id.lookup(db).scope.root_file,
            DefWithBodyId::ParamId(id) => id.lookup(db).scope.root_file,
            DefWithBodyId::FunctionId(id) => id.lookup(db).scope.root_file,
        }
    }
}
