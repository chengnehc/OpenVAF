//! See Also:
//!
//! https://docs.rs/ra_ap_hir_def/latest/ra_ap_hir_def/index.html

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use stdx::{impl_debug_display, impl_from};

use arena::Idx;
use basedb::{AstId, ErasedAstId, FileId};
use syntax::ast;
use syntax::name::Name;
use syntax::{AstNode, AstPtr};

pub mod body;
mod builtin;
mod data;
pub mod db;
pub mod expr;
mod item_tree;
pub mod nameres;
mod path;
mod types;

pub use crate::builtin::{BuiltIn, ParamSysFun};
pub use crate::data::FunctionArgData;
use crate::db::HirDefDB;
pub use crate::expr::{Case, Expr, ExprId, Literal, Stmt, StmtId};
pub use crate::item_tree::{
    AliasParam, Branch, BranchKind, Discipline, DisciplineAttr, Function, ItemTree, ItemTreeId,
    ItemTreeNode, Module, Nature, NatureAttr, NatureRef, NatureRefKind, NodeTypeDecl, Param, Var,
};
use crate::nameres::{
    DefMap, DefMapSource, PathResolveError, ResolvedPath, ScopeItemDef, ScopeItemKind,
};
pub use crate::path::Path;
pub use crate::types::Type;

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct ScopeId {
    pub root_file: FileId,
    /// which kind of `DefMap` does this scope originate from?
    pub src: DefMapSource,
    /// The scope's local ID in the `DefMap` from which it originates.
    pub local_id: nameres::LocalScopeId,
}

impl ScopeId {
    /// Return the root scope of `root_file`.
    pub fn root(root_file: FileId) -> ScopeId {
        ScopeId { root_file, src: DefMapSource::Root, local_id: 0usize.into() }
    }

    /// Return the `DefMap` of this scope.
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

            _ => self.def_map(db).resolve_normal_path_in_scope(self.local_id, &path.segments, db),
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

            _ => self.def_map(db).resolve_normal_item_path_in_scope(
                self.local_id,
                &path.segments,
                db,
            ),
        }
    }
}

#[derive(Debug)]
pub struct ItemLoc<N: ItemTreeNode> {
    pub scope: ScopeId,
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

    pub fn name(&self, db: &dyn HirDefDB) -> Name {
        N::lookup(&self.item_tree(db), self.id).name().clone()
    }

    pub fn def_map(&self, db: &dyn HirDefDB) -> Arc<DefMap> {
        self.scope.def_map(db)
    }

    pub fn source(&self, db: &dyn HirDefDB) -> N::Source {
        self.ast_ptr(db).to_node(db.parse(self.scope.root_file).tree().syntax())
    }
}

//#[allow(unknown_lints)]
//#[allow(clippy::incorrect_clone_impl_on_copy_type)]
impl<N: ItemTreeNode> Clone for ItemLoc<N> {
    fn clone(&self) -> Self {
        Self { scope: self.scope, id: self.id }
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
    type ID;
    fn intern(self, db: &dyn HirDefDB) -> Self::ID;
}
pub trait Lookup {
    type Data;
    fn lookup(&self, db: &dyn HirDefDB) -> Self::Data;
}

// Macros for implementing `Intern` and `Lookup` trait for HIR items.
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
            type ID = $id;
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
macro_rules! impl_intern {
    ($id:ident, $loc:ident, $intern:ident, $lookup:ident) => {
        impl_intern_key!($id);
        impl_intern_lookup!($id, $loc, $intern, $lookup);
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockId(salsa::InternId);
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlockLoc {
    parent: ScopeId,
    ast_id: AstId<ast::BlockStmt>,
}
impl BlockLoc {
    pub fn name(self, db: &dyn HirDefDB) -> Name {
        let tree = db.item_tree(self.parent.root_file);
        tree[self.ast_id].name.clone().expect("BlockLocs are only created for named Blocks")
    }
}
impl_intern!(BlockId, BlockLoc, intern_block, lookup_intern_block);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModuleId(salsa::InternId);
pub type ModuleLoc = ItemLoc<Module>;
impl_intern!(ModuleId, ModuleLoc, intern_module, lookup_intern_module);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisciplineId(salsa::InternId);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DisciplineLoc {
    pub root_file: FileId,
    pub id: ItemTreeId<Discipline>,
}
impl DisciplineLoc {
    pub fn item_tree(self, db: &dyn HirDefDB) -> Arc<ItemTree> {
        db.item_tree(self.root_file)
    }
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::DisciplineDecl> {
        self.item_tree(db)[self.id].ast_id
    }
    /*
    pub fn source(self, db: &dyn HirDefDB) -> ast::DisciplineDecl {
        let ast_id = self.ast_id(db);
        db.ast_id_map(self.root_file).get(ast_id).to_node(db.parse(self.root_file).tree().syntax())
    }
    */
}
impl_intern!(DisciplineId, DisciplineLoc, intern_discipline, lookup_intern_discipline);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NatureId(salsa::InternId);
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NatureLoc {
    pub root_file: FileId,
    pub id: ItemTreeId<Nature>,
}
impl NatureLoc {
    pub fn item_tree(self, db: &dyn HirDefDB) -> Arc<ItemTree> {
        db.item_tree(self.root_file)
    }
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::NatureDecl> {
        db.item_tree(self.root_file)[self.id].ast_id
    }
    /*
    pub fn source(self, db: &dyn HirDefDB) -> ast::NatureDecl {
        let ast_id = self.ast_id(db);
        db.ast_id_map(self.root_file).get(ast_id).to_node(db.parse(self.root_file).tree().syntax())
    }
    */
}
impl_intern!(NatureId, NatureLoc, intern_nature, lookup_intern_nature);

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
        let loc = self.module.lookup(db);
        loc.item_tree(db)[loc.id].nodes[self.id].ast_id
    }
    pub fn discipline_ast_id(self, db: &dyn HirDefDB) -> Option<ErasedAstId> {
        let loc = self.module.lookup(db);
        let tree = loc.item_tree(db);
        let decls = &tree[loc.id].nodes[self.id].decls;
        decls.iter().find(|decl| decl.discipline(&tree).is_some()).map(|it| it.ast_id(&tree))
    }
}
impl_debug_display!(match NodeId{ NodeId(id) => "node{:?}", id;});
impl_intern!(NodeId, NodeLoc, intern_node, lookup_intern_node);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionArgId(salsa::InternId);
pub type LocalFunctionArgId = Idx<item_tree::FunctionArg>;
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct FunctionArgLoc {
    pub fun: FunctionId,
    pub id: LocalFunctionArgId,
}
impl FunctionArgLoc {
    pub fn ast_id(self, db: &dyn HirDefDB) -> AstId<ast::FunctionArg> {
        let fun = self.fun.lookup(db);
        fun.item_tree(db)[fun.id].args[self.id].ast_ids[0]
    }
    pub fn syntax(self, db: &dyn HirDefDB) -> ast::FunctionArg {
        let file = self.fun.lookup(db).scope.root_file;
        let ptr = db.ast_id_map(file).get(self.ast_id(db));
        ptr.to_node(db.parse(file).tree().syntax())
    }
    pub fn ast_ptr(self, db: &dyn HirDefDB) -> AstPtr<ast::FunctionArg> {
        db.ast_id_map(self.fun.lookup(db).scope.root_file).get(self.ast_id(db))
    }
    pub fn name(self, db: &dyn HirDefDB) -> Name {
        let fun = self.fun.lookup(db);
        fun.item_tree(db)[fun.id].args[self.id].name.clone()
    }
}
impl_intern!(FunctionArgId, FunctionArgLoc, intern_function_arg, lookup_intern_function_arg);

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
        let item_tree = &nature.item_tree(db);
        let nature = &item_tree[nature.id];
        let id = u32::from(nature.attrs.start()) + u32::from(self.id);
        let id: ItemTreeId<item_tree::NatureAttr> = ItemTreeId::from(id);
        item_tree[id].ast_id()
    }
}
impl_intern!(NatureAttrId, NatureAttrLoc, intern_nature_attr, lookup_intern_nature_attr);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefWithBodyId {
    ModuleId { initial: bool, module: ModuleId },
    ParamId(ParamId),
    VarId(VarId),
    FunctionId(FunctionId),
    NatureAttrId(NatureAttrId),
    DisciplineAttrId(DisciplineAttrId),
}

impl_from!(ParamId, FunctionId, VarId, NatureAttrId, DisciplineAttrId for DefWithBodyId);

impl TryFrom<ScopeItemDef> for DefWithBodyId {
    type Error = ();
    fn try_from(src: ScopeItemDef) -> Result<DefWithBodyId, ()> {
        let res = match src {
            ScopeItemDef::VarId(var) => var.into(),
            ScopeItemDef::ParamId(param) => param.into(),
            ScopeItemDef::FunctionId(fun) => fun.into(),
            ScopeItemDef::NatureAttrId(attr) => attr.into(),
            _ => return Err(()),
        };
        Ok(res)
    }
}

impl DefWithBodyId {
    pub fn file(self, db: &dyn HirDefDB) -> FileId {
        match self {
            DefWithBodyId::ModuleId { module, .. } => module.lookup(db).scope.root_file,
            DefWithBodyId::ParamId(id) => id.lookup(db).scope.root_file,
            DefWithBodyId::FunctionId(id) => id.lookup(db).scope.root_file,
            DefWithBodyId::VarId(id) => id.lookup(db).scope.root_file,
            DefWithBodyId::NatureAttrId(id) => id.lookup(db).nature.lookup(db).root_file,
            DefWithBodyId::DisciplineAttrId(id) => id.lookup(db).discipline.lookup(db).root_file,
        }
    }
}
