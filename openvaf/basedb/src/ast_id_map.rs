//! `AstIdMap` allows to create stable IDs for "large" syntax nodes like items
//! and macro calls.
//!
//! Specifically, it enumerates all items in a file and uses position of a an
//! item as an ID. That way, ID's don't change unless the set of items itself
//! changes.
//!
//! See Also:
//!
//! https://github.com/rust-lang/rust-analyzer/blob/master/crates/span/src/ast_id.rs

use std::any::type_name;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;
use std::ops::Deref;
use std::{fmt, iter};

use arena::{Arena, ArenaMap, Idx, RawIdx};
use syntax::name::{AsName, Name};
use syntax::{ast, AstNode, AstPtr, SyntaxKind, SyntaxNode, SyntaxNodePtr};

use crate::{BaseDB, FileId};

#[derive(Debug, PartialEq, Eq, Default)]
pub struct AstIdMap {
    arena: Arena<MapEntry>,
    /// reverse mapping for lint attribute resolution
    parents: ArenaMap<MapEntry, Option<Idx<MapEntry>>>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct MapEntry {
    pub(crate) syntax: SyntaxNodePtr,
    pub(crate) attrs: Box<[Name]>,
}

/// Type-erased `AstId`
pub type ErasedAstId = Idx<MapEntry>;

/// `AstId` wraps the underlying type-erased `ErasedAstId`.
pub struct AstId<N: AstNode> {
    raw: ErasedAstId,
    _ty: PhantomData<fn() -> N>,
}

impl<N: AstNode> AstId<N> {
    pub fn erased(&self) -> ErasedAstId {
        self.raw
    }

    // Can't make this a From implementation because of coherence
    pub fn upcast<M: AstNode>(self) -> AstId<M>
    where
        N: Into<M>,
    {
        AstId { raw: self.raw, _ty: PhantomData }
    }
}

impl<N: AstNode> From<AstId<N>> for ErasedAstId {
    fn from(id: AstId<N>) -> Self {
        id.raw
    }
}

impl<N: AstNode> Clone for AstId<N> {
    fn clone(&self) -> AstId<N> {
        *self
    }
}
impl<N: AstNode> Copy for AstId<N> {}

impl<N: AstNode> PartialEq for AstId<N> {
    fn eq(&self, other: &Self) -> bool {
        self.raw == other.raw
    }
}
impl<N: AstNode> Eq for AstId<N> {}

impl<N: AstNode> Hash for AstId<N> {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.raw.hash(hasher);
    }
}

impl<N: AstNode> fmt::Debug for AstId<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AstId::<{}>({})", type_name::<N>(), RawIdx::from(self.raw))
    }
}

/// Does this kind of CST node have an entry in `AstIdMap`?
pub(crate) fn has_map_entry(kind: SyntaxKind) -> bool {
    if ast::BodyPortDecl::can_cast(kind) {
        // This just adds a semicolon to a port decl... No need to add the same port twice
        false
    } else {
        ast::Item::can_cast(kind)
            || ast::Param::can_cast(kind)
            || ast::Var::can_cast(kind)
            || ast::FunctionArg::can_cast(kind)
            || ast::NatureAttr::can_cast(kind)
            || ast::DisciplineAttr::can_cast(kind)
            || ast::ModuleItem::can_cast(kind)
            || ast::ModulePort::can_cast(kind)
            || ast::PortDecl::can_cast(kind)
            || ast::AnalogBehaviour::can_cast(kind)
            || ast::BlockStmt::can_cast(kind)
    }
}

impl AstIdMap {
    /// Create a `AstIdMap` from source file CST node.
    pub(crate) fn from_source(node: &SyntaxNode) -> AstIdMap {
        assert!(node.parent().is_none());
        let mut res = AstIdMap::default();
        // By walking the tree in breadth-first order we make sure that parents
        // get lower ids then children. That is, adding a new child does not
        // change parent's id. This means that, say, adding a new function to a
        // module does not change ids of top-level items, which helps caching.
        //
        // Compared to rust analyzer, the 'parent' mapping was added here for
        // lint attribute resolution.
        //
        // TODO does this hurt caching in any way?
        // Probably not: if the parent changes then something before changed
        // as well and the item is moved anyway.
        bdfs(node, |it, parent| has_map_entry(it.kind()).then(|| res.alloc(it, parent)));
        res
    }

    #[inline]
    pub(crate) fn entries(&self) -> impl Iterator<Item = (ErasedAstId, &MapEntry)> {
        self.arena.iter_enumerated()
    }

    pub fn id_of<N: AstNode>(&self, item: &N) -> AstId<N> {
        let raw = self.erased_ast_id(item.syntax());
        AstId { raw, _ty: PhantomData }
    }

    pub fn get<N: AstNode>(&self, id: AstId<N>) -> AstPtr<N> {
        self.arena[id.raw].syntax.cast::<N>().unwrap()
    }

    pub fn get_erased(&self, id: ErasedAstId) -> SyntaxNodePtr {
        self.arena[id].syntax
    }

    #[inline]
    fn erased_ast_id(&self, node: &SyntaxNode) -> ErasedAstId {
        let ptr = SyntaxNodePtr::new(node);
        self.erased_ast_id_for_ptr(ptr)
    }

    #[inline]
    fn erased_ast_id_for_ptr(&self, ptr: SyntaxNodePtr) -> ErasedAstId {
        let Some((id, _)) = self.entries().find(|(_id, e)| e.syntax == ptr) else {
            panic!("Can't find {:?} in AstIdMap", ptr)
        };
        id
    }

    /// Get the position of attribute with `name` of AST node `id`, if any.
    pub fn get_attr<N: AstNode>(&self, id: AstId<N>, name: &str) -> Option<usize> {
        self.arena[id.raw].attrs.iter().position(|attr| attr.deref() == name)
    }

    pub(crate) fn get_parent(&self, id: ErasedAstId) -> Option<ErasedAstId> {
        self.parents[id]
    }

    pub fn nearest_ast_id_to_ptr(
        &self,
        ptr: SyntaxNodePtr,
        db: &dyn BaseDB,
        root_file: FileId,
    ) -> Option<ErasedAstId> {
        if has_map_entry(ptr.syntax_kind()) {
            Some(self.erased_ast_id_for_ptr(ptr))
        } else {
            let node = ptr.to_node(db.parse(root_file).tree().syntax());
            self.nearest_ast_id_to_node(node)
        }
    }

    pub(crate) fn nearest_ast_id_to_node(&self, mut node: SyntaxNode) -> Option<ErasedAstId> {
        while !has_map_entry(node.kind()) {
            node = node.parent()?;
        }
        Some(self.erased_ast_id(&node))
    }

    fn alloc(&mut self, item: &SyntaxNode, parent: Option<ErasedAstId>) -> ErasedAstId {
        let id1 = self.parents.push_and_get_key(parent);
        let attrs = if ast::Param::can_cast(item.kind()) || ast::Var::can_cast(item.kind()) {
            ast::attrs(&item.parent().unwrap())
        } else {
            ast::attrs(item)
        };
        let attrs = attrs.filter_map(|attr| Some(attr.name()?.as_name())).collect();
        let id2 = self.arena.push_and_get_key(MapEntry { syntax: SyntaxNodePtr::new(item), attrs });
        debug_assert_eq!(id1, id2);
        id2
    }
}

/// Walks the subtree in bdfs order, calling `f` for each node. What is bdfs
/// order? It is a mix of breadth-first and depth first orders. Nodes for which
/// `f` returns true are visited breadth-first, all the other nodes are explored
/// depth-first.
///
/// In other words, the size of the bfs queue is bound by the number of "true"
/// nodes.
fn bdfs(
    node: &SyntaxNode,
    mut f: impl FnMut(&SyntaxNode, Option<ErasedAstId>) -> Option<ErasedAstId>,
) {
    let mut curr_layer = vec![(node.clone(), None)];
    let mut next_layer = vec![];
    while !curr_layer.is_empty() {
        curr_layer.drain(..).for_each(|(node, id)| {
            let mut preorder = node.preorder();
            while let Some(event) = preorder.next() {
                match event {
                    syntax::WalkEvent::Enter(node) => {
                        if let Some(it) = f(&node, id) {
                            next_layer.extend(node.children().zip(iter::repeat(Some(it))));
                            preorder.skip_subtree();
                        }
                    }
                    syntax::WalkEvent::Leave(_) => {}
                }
            }
        });
        std::mem::swap(&mut curr_layer, &mut next_layer);
    }
}
