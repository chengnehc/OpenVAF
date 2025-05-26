//! In OpenVAF syntax trees are transient objects. That means that we create
//! trees when we need them, and tear them down to save memory.
//!
//! In this architecture, hanging on to a particular syntax node for a long
//! time is ill-advisable, as that keeps the whole tree resident.
//!
//! Instead, we provide a [`SyntaxNodePtr`] type, which stores information about
//! *location* of a particular syntax node in a tree. It's a small type which can
//! be cheaply stored, and which can be resolved to a real [`SyntaxNode`] when
//! necessary.

use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use crate::syntax_node::VerilogALanguage;
use crate::{AstNode, SyntaxKind, SyntaxNode, TextRange};

/// A "pointer" to a `SyntaxNode`, via location in the source code.
/// It can be used to remember a specific node across reparses of the same file.
///
/// It's a small type which can be cheaply stored, and which can be resolved
/// to a real [`SyntaxNode`] when necessary.
///
pub type SyntaxNodePtr = rowan::ast::SyntaxNodePtr<VerilogALanguage>;

/// Like `SyntaxNodePtr`, but remembers the type of node
#[derive(Debug)]
pub struct AstPtr<N: AstNode> {
    raw: SyntaxNodePtr,
    _ty: PhantomData<fn() -> N>,
}

impl<N: AstNode> Clone for AstPtr<N> {
    fn clone(&self) -> AstPtr<N> {
        AstPtr { raw: self.raw, _ty: PhantomData }
    }
}

impl<N: AstNode> Eq for AstPtr<N> {}

impl<N: AstNode> PartialEq for AstPtr<N> {
    fn eq(&self, other: &AstPtr<N>) -> bool {
        self.raw == other.raw
    }
}

impl<N: AstNode> Hash for AstPtr<N> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.raw.hash(state)
    }
}

impl<N: AstNode> AstPtr<N> {
    #[inline]
    pub fn new(node: &N) -> AstPtr<N> {
        AstPtr { raw: SyntaxNodePtr::new(node.syntax()), _ty: PhantomData }
    }

    /// "Dereference" the pointer to get the node it points to.
    #[inline]
    pub fn to_node(&self, root: &SyntaxNode) -> N {
        let syntax_node = self.raw.to_node(root);
        N::cast(syntax_node).unwrap()
    }

    /// Cast the `AstPtr` to point to type `U`, if possible.
    #[inline]
    pub fn cast<U: AstNode>(self) -> Option<AstPtr<U>> {
        if !U::can_cast(self.raw.kind()) {
            return None;
        }
        Some(AstPtr { raw: self.raw, _ty: PhantomData })
    }

    /// Like `SyntaxNodePtr::cast` but the trait bounds work out.
    pub fn try_from_raw(raw: SyntaxNodePtr) -> Option<AstPtr<N>> {
        N::can_cast(raw.kind()).then_some(AstPtr { raw, _ty: PhantomData })
    }

    #[inline]
    pub fn text_range(&self) -> TextRange {
        self.raw.text_range()
    }

    #[inline]
    pub fn syntax_kind(&self) -> SyntaxKind {
        self.raw.kind()
    }
}

impl<N: AstNode> From<AstPtr<N>> for SyntaxNodePtr {
    fn from(ptr: AstPtr<N>) -> SyntaxNodePtr {
        ptr.raw
    }
}
