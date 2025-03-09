//! A simplified AST that only contains items.
//!
//! This is the primary IR used throughout `hir_def` and input to name resolution algorithms.
//!
//! One important purpose of this layer is to provide an "invalidation barrier" for incremental
//! computations: when typing inside an item body, the `ItemTree` of the modified file is typically
//! unaffected, so we don't have to recompute name resolution results or item data (see `data.rs`).
//!
//! In general, any item in the `ItemTree` stores its `AstId`, which allows mapping it back to its
//! surface syntax.

use std::hash::Hash;
use std::ops::Index;
use std::sync::Arc;
use stdx::impl_from_typed;

use ahash::AHashMap;
use arena::{Arena, Idx, IdxRange};
use basedb::{AstId, ErasedAstId, FileId};
use syntax::ast::{self, BlockStmt, NameRef};
use syntax::name::Name;
use syntax::AstNode;
use typed_index_collections::TiVec;

use crate::db::HirDefDB;
use crate::{
    LocalDisciplineAttrId, LocalFunctionArgId, LocalNatureAttrId, LocalNodeId, Path, Type,
};

mod lower;
mod pretty;

/// An item tree is a simplified AST that only contains items.
#[derive(Debug, Eq, PartialEq, Default)]
pub struct ItemTree {
    pub top_level: Box<[RootItem]>,

    pub(crate) data: ItemTreeData,
    /// mapping from block statement AstId to a block
    pub(crate) blocks: AHashMap<AstId<BlockStmt>, Block>,
}

impl ItemTree {
    pub(crate) fn query(db: &dyn HirDefDB, file: FileId) -> Arc<ItemTree> {
        let syntax_tree = db.parse(file).tree();
        let ctxt = lower::Context::new(db, file);
        let mut item_tree = ctxt.lower_root_items(&syntax_tree);
        item_tree.shrink_to_fit();

        Arc::new(item_tree)
    }

    fn shrink_to_fit(&mut self) {
        let ItemTreeData {
            natures,
            nature_attrs,
            disciplines,
            discipline_attrs,
            modules,
            ports,
            nets,
            branches,
            variables,
            parameters,
            aliasparams,
            functions,
        } = &mut self.data;
        natures.shrink_to_fit();
        nature_attrs.shrink_to_fit();
        disciplines.shrink_to_fit();
        discipline_attrs.shrink_to_fit();
        modules.shrink_to_fit();
        ports.shrink_to_fit();
        nets.shrink_to_fit();
        branches.shrink_to_fit();
        variables.shrink_to_fit();
        parameters.shrink_to_fit();
        aliasparams.shrink_to_fit();
        functions.shrink_to_fit();
    }
}

/// An item that is defined at top level of source file (in the root scope),
/// i.e. `discipline`, `nature` and `module`
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum RootItem {
    Nature(ItemTreeId<Nature>),
    Discipline(ItemTreeId<Discipline>),
    Module(ItemTreeId<Module>),
}
impl_from_typed! (
    Nature(ItemTreeId<Nature>),
    Discipline(ItemTreeId<Discipline>),
    Module(ItemTreeId<Module>)
    for RootItem
);

#[derive(Default, Debug, Eq, PartialEq)]
pub(crate) struct ItemTreeData {
    // # Note
    // Disciplines or Natures share the same arena of their attributes
    // within which each discipline or nature owns a `IdxRange` of attributes.
    pub natures: Arena<Nature>,
    pub nature_attrs: Arena<NatureAttr>,
    pub disciplines: Arena<Discipline>,
    pub discipline_attrs: Arena<DisciplineAttr>,
    pub modules: Arena<Module>,
    pub ports: Arena<Port>,
    pub nets: Arena<Net>,
    pub branches: Arena<Branch>,
    pub variables: Arena<Var>,
    pub parameters: Arena<Param>,
    pub aliasparams: Arena<AliasParam>,
    pub functions: Arena<Function>,
}

pub type ItemTreeId<N> = Idx<N>;

/// Trait implemented by all nodes in the item tree.
pub trait ItemTreeNode: Clone {
    // This means: the trait has an associative type `Source`
    // and it must satisfy trait bound `AstNode`.
    type Source: AstNode;

    /// The name of this item.
    fn name(&self) -> &Name;
    /// The `AstId` of this item, allowing to map it back to its surface syntax.
    fn ast_id(&self) -> AstId<Self::Source>;
    /// Looks up an item with this type in the tree.
    fn lookup(tree: &ItemTree, index: ItemTreeId<Self>) -> &Self;
}

macro_rules! item_tree_nodes {
    ( $( $typ:ident in $fld:ident -> $ast:ty ),+ $(,)? ) => {
        // generic item type
        #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
        pub enum ScopeItem {
            $( $typ(ItemTreeId<$typ>), )+
        }
        $( // conversion between generic and typed items
        impl From<ItemTreeId<$typ>> for ScopeItem {
            fn from(id: ItemTreeId<$typ>) -> ScopeItem {
                ScopeItem::$typ(id)
            }
        }
        impl TryFrom<ScopeItem> for ItemTreeId<$typ> {
            type Error = ();

            fn try_from(it: ScopeItem) -> Result<ItemTreeId<$typ>, ()> {
                if let ScopeItem::$typ(id) = it {
                    Ok(id)
                } else {
                    Err(())
                }
            }
        }
        )+
        $( // `ItemTreeNode` trait impls
        impl ItemTreeNode for $typ {
            type Source = $ast;
            #[inline]
            fn name(&self) -> &Name {
                &self.name
            }
            #[inline]
            fn ast_id(&self) -> AstId<Self::Source> {
                self.ast_id
            }
            #[inline]
            fn lookup(tree: &ItemTree, index: Idx<Self>) -> &Self {
                &tree.data.$fld[index]
            }
        }
        // [] operator overload of each arena of item tree data
        impl Index<Idx<$typ>> for ItemTree {
            type Output = $typ;

            fn index(&self, index: Idx<$typ>) -> &Self::Output {
                &self.data.$fld[index]
            }
        }
        )+
    };
}

item_tree_nodes! {
    Nature in natures -> ast::NatureDecl,
    NatureAttr in nature_attrs -> ast::NatureAttr,
    Discipline in disciplines -> ast::DisciplineDecl,
    DisciplineAttr in discipline_attrs -> ast::DisciplineAttr,
    Module in modules -> ast::ModuleDecl,
    Port in ports -> ast::PortDecl,
    Net in nets -> ast::NetDecl,
    Branch in branches -> ast::BranchDecl,
    Var in variables -> ast::Var,
    Param in parameters -> ast::Param,
    AliasParam in aliasparams -> ast::AliasParam,
    Function in functions -> ast::Function,
}

/* Items that impls `ItemTreeNode` trait */

/// [LRM 3.6.1] A nature is a collection of attributes.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Nature {
    pub name: Name,
    pub parent: Option<NatureRef>,
    // Predefined nature attributes. 'units', 'access', 'abstol' are required
    // for base nature, that is, natures not derived from any other nature.
    pub units: Option<(String, LocalNatureAttrId)>,
    pub access: Option<(Name, LocalNatureAttrId)>,
    // TODO(JW) abstol is not fully supported
    pub abstol: Option<LocalNatureAttrId>,
    pub ddt_nature: Option<(NatureRef, LocalNatureAttrId)>,
    pub idt_nature: Option<(NatureRef, LocalNatureAttrId)>,
    // All attributes: pre-defined + user-defined
    pub attrs: IdxRange<NatureAttr>,
    pub ast_id: AstId<ast::NatureDecl>,
}
/// [LRM 3.6.1.1] A derived nature can declare additional attributes or override attribute
/// values of the parent nature, with certain restrictions for the predefined attributes.
///
/// [LRM 3.6.2.6] A nature can be derived from the nature bound to the potential or flow
/// in a discipline.
#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct NatureRef {
    pub name: Name,
    pub kind: NatureRefKind,
}
#[derive(Debug, Eq, PartialEq, Clone, Hash, Copy)]
pub enum NatureRefKind {
    Nature,
    DisciplinePotential,
    DisciplineFlow,
}
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct NatureAttr {
    pub name: Name,
    pub ast_id: AstId<ast::NatureAttr>,
}

/// [LRM 3.6.2] A discipline description consists of specifying a domain type and binding any
/// natures to potential or flow.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Discipline {
    pub name: Name,
    // nature binding
    pub potential: Option<(NatureRef, LocalDisciplineAttrId)>,
    pub flow: Option<(NatureRef, LocalDisciplineAttrId)>,
    // domain binding
    pub domain: Option<(Domain, LocalDisciplineAttrId)>,
    // All attributes: pre-defined + user-defined
    pub attrs: IdxRange<DisciplineAttr>,
    pub ast_id: AstId<ast::DisciplineDecl>,
}
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Domain {
    Discrete,
    Continuous,
}
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DisciplineAttr {
    pub name: Name,
    pub kind: DisciplineAttrKind,
    pub ast_id: AstId<ast::DisciplineAttr>,
}
#[derive(Debug, Eq, PartialEq, Clone, Hash, Copy)]
pub enum DisciplineAttrKind {
    FlowOverride,
    PotentialOverride,
    UserDefined,
}

/// [LRM 6.2]
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Module {
    pub name: Name,
    pub num_ports: u32,
    pub nodes: TiVec<LocalNodeId, Node>,
    pub items: Vec<ModuleItem>,
    pub ast_id: AstId<ast::ModuleDecl>,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum ModuleItem {
    Node(LocalNodeId),
    Branch(ItemTreeId<Branch>),
    Variable(ItemTreeId<Var>),
    Parameter(ItemTreeId<Param>),
    AliasParam(ItemTreeId<AliasParam>),
    Function(ItemTreeId<Function>),
    Block(AstId<BlockStmt>),
}
impl_from_typed! (
    Node(LocalNodeId),
    Branch(ItemTreeId<Branch>),
    Variable(ItemTreeId<Var>),
    Parameter(ItemTreeId<Param>),
    AliasParam(ItemTreeId<AliasParam>),
    Function(ItemTreeId<Function>),
    Block(AstId<BlockStmt>) for ModuleItem
);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Port {
    pub name: Name,
    pub name_idx: usize,
    pub discipline: Option<Name>,
    pub is_input: bool,
    pub is_output: bool,
    pub is_gnd: bool,
    pub ast_id: AstId<ast::PortDecl>,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Net {
    pub name: Name,
    pub name_idx: usize,
    pub discipline: Option<Name>,
    pub is_gnd: bool,
    pub ast_id: AstId<ast::NetDecl>,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Branch {
    pub name: Name,
    pub name_idx: usize,
    pub kind: BranchKind,
    pub ast_id: AstId<ast::BranchDecl>,
}
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum BranchKind {
    Nodes(Path, Path),
    NodeGnd(Path),
    PortFlow(Path),
    Missing,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Var {
    pub name: Name,
    pub ty: Type,
    pub ast_id: AstId<ast::Var>,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Param {
    pub name: Name,
    pub ty: Option<Type>,
    pub is_local: bool,
    pub ast_id: AstId<ast::Param>,
}

#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct AliasParam {
    pub name: Name,
    pub src: Option<Path>,
    pub ast_id: AstId<ast::AliasParam>,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Function {
    pub name: Name,
    pub ty: Type,
    pub args: TiVec<LocalFunctionArgId, FunctionArg>,
    pub items: Vec<FunctionItem>,
    pub ast_id: AstId<ast::Function>,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum FunctionItem {
    Block(AstId<BlockStmt>),
    Parameter(ItemTreeId<Param>),
    Variable(ItemTreeId<Var>),
    FunctionArg(LocalFunctionArgId),
}
impl_from_typed! (
    Block(AstId<BlockStmt>),
    Parameter(ItemTreeId<Param>),
    Variable(ItemTreeId<Var>),
    FunctionArg(LocalFunctionArgId) for FunctionItem
);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct FunctionArg {
    pub name: Name,
    pub name_idx: usize,
    pub is_input: bool,
    pub is_output: bool,
    pub declarations: Vec<ItemTreeId<Var>>,
    pub ast_ids: Vec<AstId<ast::FunctionArg>>,
}
impl FunctionArg {
    pub fn ty(&self, tree: &ItemTree) -> Type {
        self.declarations.first().map_or(Type::Err, |decl| tree[*decl].ty.clone())
    }
}

/// `Node` is an abstraction over `Net` and `Port`. A `Node` may be defined multiple
/// times as `Port` or `Net` (abstracted by `NodeTypeDecl`).
///
/// `NodeTypeDecl` cannot be mapped to a concrete ast node, so `ErasedAstId` is used.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Node {
    pub name: Name,
    pub is_port: bool,
    pub decls: Vec<NodeTypeDecl>, // TODO small vec?
    pub ast_id: ErasedAstId,
}
impl Node {
    pub fn discipline(&self, tree: &ItemTree) -> Option<Name> {
        self.decls.iter().find_map(|decl| decl.discipline(tree).clone())
    }

    pub fn is_gnd(&self, tree: &ItemTree) -> bool {
        self.decls.iter().any(|decl| decl.is_gnd(tree))
    }

    pub fn direction(&self, tree: &ItemTree) -> (bool, bool) {
        match self.decls.iter().find_map(|decl| decl.direction(tree)) {
            Some(direction) => direction,
            // default to inout to avoid confusing error messages
            // and to allow omitting direction specification
            // for backwards compatibility with cadence, see issue #40
            None if self.is_port => (true, true),
            None => (false, false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Copy, Hash)]
pub enum NodeTypeDecl {
    Net(ItemTreeId<Net>),
    Port(ItemTreeId<Port>),
}
impl_from_typed!(
    Net(ItemTreeId<Net>),
    Port(ItemTreeId<Port>) for NodeTypeDecl
);
impl NodeTypeDecl {
    pub fn discipline(self, tree: &ItemTree) -> &Option<Name> {
        match self {
            NodeTypeDecl::Net(net) => &tree[net].discipline,
            NodeTypeDecl::Port(port) => &tree[port].discipline,
        }
    }

    pub fn discipline_source(self, db: &dyn HirDefDB, root_file: FileId) -> Option<NameRef> {
        let ast_id_map = db.ast_id_map(root_file);
        let tree = db.item_tree(root_file);
        let ast = db.parse(root_file).syntax_node();
        match self {
            NodeTypeDecl::Net(net) => ast_id_map.get(tree[net].ast_id).to_node(&ast).discipline(),
            NodeTypeDecl::Port(port) => {
                ast_id_map.get(tree[port].ast_id).to_node(&ast).discipline()
            }
        }
    }

    pub fn name<'a>(&self, tree: &'a ItemTree) -> &'a Name {
        match *self {
            NodeTypeDecl::Net(net) => &tree[net].name,
            NodeTypeDecl::Port(port) => &tree[port].name,
        }
    }

    pub fn is_gnd(self, tree: &ItemTree) -> bool {
        match self {
            NodeTypeDecl::Net(net) => tree[net].is_gnd,
            NodeTypeDecl::Port(port) => tree[port].is_gnd,
        }
    }

    pub fn direction(self, tree: &ItemTree) -> Option<(bool, bool)> {
        match self {
            NodeTypeDecl::Port(port) => Some((tree[port].is_input, tree[port].is_output)),
            NodeTypeDecl::Net(_) => None,
        }
    }

    pub fn ast_id(self, tree: &ItemTree) -> ErasedAstId {
        match self {
            NodeTypeDecl::Net(net) => tree[net].ast_id.into(),
            NodeTypeDecl::Port(port) => tree[port].ast_id.into(),
        }
    }
}

/// [LRM 5.3] Block Statements
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Block {
    pub name: Option<Name>,
    pub block_items: Vec<BlockItem>,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum BlockItem {
    Block(AstId<BlockStmt>),
    Parameter(ItemTreeId<Param>),
    Variable(ItemTreeId<Var>),
}
impl_from_typed! (
    Block(AstId<BlockStmt>),
    Parameter(ItemTreeId<Param>),
    Variable(ItemTreeId<Var>) for BlockItem
);

impl Index<AstId<BlockStmt>> for ItemTree {
    type Output = Block;

    fn index(&self, index: AstId<BlockStmt>) -> &Self::Output {
        &self.blocks[&index]
    }
}
