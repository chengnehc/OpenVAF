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

use std::ops::Index;
use std::sync::Arc;
use stdx::impl_from_typed;

use ahash::AHashMap;
use arena::{Arena, Idx, IdxRange};
use basedb::{AstId, ErasedAstId, FileId};
use syntax::ast::{self, BlockStmt};
use syntax::name::Name;
use syntax::AstNode;

use crate::HirDefDB;

mod lower;
mod nodes;
mod pretty;

pub use nodes::*;

/// An item tree is a simplified AST that only contains items.
#[derive(Debug, Eq, PartialEq, Default)]
pub struct ItemTree {
    pub(crate) top_level: Box<[RootItem]>,
    /// The data storage for items.
    pub(crate) data: ItemTreeData,
    /// Map from block statement AstId to the block's data. This special treatment is taken as
    /// `BlockStmt` is not an item tree node.
    // TODO(JW): currently all blocks (named and unnamed) are stored. Is that really necessary?
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
    Module(ItemTreeId<Module>)  for RootItem
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

pub trait ItemTreeNode: Clone {
    // This means trait has an associative type `Source` that must satisfy trait bound `AstNode`.
    type Source: AstNode;

    /// The name of this item.
    fn name(&self) -> &Name;
    /// The `AstId` of this item, allowing to map it back to its surface syntax.
    fn ast_id(&self) -> AstId<Self::Source>;
    /// Looks up an item of this type in the item tree.
    fn lookup(tree: &ItemTree, index: ItemTreeId<Self>) -> &Self;
}

macro_rules! scope_items {
    ($typ:ident) => {
        impl From<ItemTreeId<$typ>> for ScopeItem {
            fn from(id: ItemTreeId<$typ>) -> ScopeItem {
                ScopeItem::$typ(id)
            }
        }
        impl TryFrom<ScopeItem> for ItemTreeId<$typ> {
            type Error = (); // TODO(JW): should not use () as Error type
            fn try_from(it: ScopeItem) -> Result<ItemTreeId<$typ>, ()> {
                if let ScopeItem::$typ(id) = it {
                    Ok(id)
                } else {
                    Err(())
                }
            }
        }
    };
}
macro_rules! item_tree_nodes {
    ( $( $typ:ident in $fld:ident -> $ast:ty ),+ $(,)? ) => {
        #[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
        pub enum ScopeItem {
            $( $typ(ItemTreeId<$typ>), )+
        }
        $(
            scope_items!($typ);
             // `ItemTreeNode` trait impls
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
