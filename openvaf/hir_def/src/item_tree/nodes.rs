use super::*;

use crate::{
    LocalDisciplineAttrId, LocalFunctionArgId, LocalNatureAttrId, LocalNodeId, Path, Type,
};
use syntax::{ast::BlockStmt, SyntaxNodePtr};

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Nature {
    pub name: Name,
    pub ast_id: AstId<ast::NatureDecl>,
    pub parent: Option<NatureRef>,
    // Predefined nature attributes
    // 'units', 'access', 'abstol' are required for base nature
    pub units: Option<(String, LocalNatureAttrId)>,
    pub access: Option<(Name, LocalNatureAttrId)>,
    pub abstol: Option<LocalNatureAttrId>, // abstol is not fully supported
    // 'ddt_nature' and 'idt_nature' are optional
    pub ddt_nature: Option<(NatureRef, LocalNatureAttrId)>,
    pub idt_nature: Option<(NatureRef, LocalNatureAttrId)>,
    // index range of attributes in the shared arena
    pub attrs: IdxRange<NatureAttr>,
}

#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct NatureRef {
    pub name: Name,
    pub kind: NatureRefKind,
    pub src: SyntaxNodePtr,
}
/// [LRM 3.6.1.1]
/// [LRM 3.6.2.6]
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

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Discipline {
    pub name: Name,
    pub ast_id: AstId<ast::DisciplineDecl>,
    // nature binding
    pub potential: Option<(NatureRef, LocalDisciplineAttrId)>,
    pub flow: Option<(NatureRef, LocalDisciplineAttrId)>,
    // domain binding
    pub domain: Option<(Domain, LocalDisciplineAttrId)>,
    // index range of attributes in the shared arena
    pub attrs: IdxRange<DisciplineAttr>,
}
#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum Domain {
    Discrete,
    Continuous,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct DisciplineAttr {
    pub name: Name,
    pub ast_id: AstId<ast::DisciplineAttr>,
    pub kind: DisciplineAttrKind,
}
/// [LRM 3.6.2.5]
#[derive(Debug, Eq, PartialEq, Clone, Hash, Copy)]
pub enum DisciplineAttrKind {
    FlowOverride,
    PotentialOverride,
    UserDefined,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Module {
    pub name: Name,
    pub ast_id: AstId<ast::ModuleDecl>,
    pub num_ports: u32,
    pub nodes: Arena<Node>,
    pub items: Vec<ModuleItem>,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum ModuleItem {
    Node(LocalNodeId),
    Branch(ItemTreeId<Branch>),
    Variable(ItemTreeId<Var>),
    Parameter(ItemTreeId<Param>),
    AliasParam(ItemTreeId<AliasParam>),
    Function(ItemTreeId<Function>),
    ScopedBlock(AstId<BlockStmt>),
}
impl_from_typed! (
    Node(LocalNodeId),
    Branch(ItemTreeId<Branch>),
    Variable(ItemTreeId<Var>),
    Parameter(ItemTreeId<Param>),
    AliasParam(ItemTreeId<AliasParam>),
    Function(ItemTreeId<Function>),
    ScopedBlock(AstId<BlockStmt>)   for ModuleItem
);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Port {
    pub name_idx: usize,
    pub name: Name,
    pub ast_id: AstId<ast::PortDecl>,
    pub discipline: Option<Name>,
    pub is_input: bool,
    pub is_output: bool,
    pub is_gnd: bool,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Net {
    pub name_idx: usize,
    pub name: Name,
    pub ast_id: AstId<ast::NetDecl>,
    pub discipline: Option<Name>,
    pub is_gnd: bool,
}

/// `Node` is an abstraction over `Net` and `Port`. A `Node` may be defined multiple
/// times as `Port` or `Net` (abstracted by `NodeTypeDecl`).
///
/// Since `NodeTypeDecl` cannot be mapped to a concrete ast node, `ErasedAstId` is used.
#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Node {
    pub name: Name,
    pub ast_id: ErasedAstId,
    pub is_port: bool,
    pub decls: Vec<NodeTypeDecl>, // TODO small vec?
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
    pub fn discipline_source(self, db: &dyn HirDefDB, root_file: FileId) -> SyntaxNodePtr {
        let ast_id_map = db.ast_id_map(root_file);
        let tree = db.item_tree(root_file);
        let cst = db.parse(root_file).root();
        match self {
            NodeTypeDecl::Net(net) => {
                let node = ast_id_map.get(tree[net].ast_id).to_node(&cst).discipline().unwrap();
                SyntaxNodePtr::new(node.syntax())
            }
            NodeTypeDecl::Port(port) => {
                let node = ast_id_map.get(tree[port].ast_id).to_node(&cst).discipline().unwrap();
                SyntaxNodePtr::new(node.syntax())
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

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Branch {
    pub name_idx: usize,
    pub name: Name,
    pub ast_id: AstId<ast::BranchDecl>,
    pub kind: BranchKind,
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
    pub ast_id: AstId<ast::Var>,
    pub ty: Type,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Param {
    pub name: Name,
    pub ast_id: AstId<ast::Param>,
    pub ty: Option<Type>,
    pub is_local: bool, // for localparam
}

#[derive(Debug, Eq, PartialEq, Clone, Hash)]
pub struct AliasParam {
    pub name: Name,
    pub ast_id: AstId<ast::AliasParam>,
    pub param_ref: Name,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Function {
    pub name: Name,
    pub ast_id: AstId<ast::Function>,
    pub ty: Type,
    pub args: Arena<FunctionArg>,
    pub items: Vec<FunctionItem>,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum FunctionItem {
    FunctionArg(LocalFunctionArgId),
    Parameter(ItemTreeId<Param>),
    Variable(ItemTreeId<Var>),
}
impl_from_typed! (
    FunctionArg(LocalFunctionArgId),
    Parameter(ItemTreeId<Param>),
    Variable(ItemTreeId<Var>)      for FunctionItem
);

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct FunctionArg {
    pub name_idx: usize,
    pub name: Name,
    pub ast_id: AstId<ast::FunctionArg>,
    pub is_input: bool,
    pub is_output: bool,
    pub var_binds: Vec<ItemTreeId<Var>>,
}
impl FunctionArg {
    pub fn ty(&self, tree: &ItemTree) -> Type {
        self.var_binds.first().map_or(Type::Err, |var| tree[*var].ty.clone())
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct Block {
    pub name: Option<Name>,
    pub items: Vec<BlockItem>,
}
#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum BlockItem {
    ScopedBlock(AstId<BlockStmt>),
    Parameter(ItemTreeId<Param>),
    Variable(ItemTreeId<Var>),
}
impl_from_typed! (
    ScopedBlock(AstId<BlockStmt>),
    Parameter(ItemTreeId<Param>),
    Variable(ItemTreeId<Var>)       for BlockItem
);
