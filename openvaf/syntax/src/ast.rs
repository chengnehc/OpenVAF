use std::marker::PhantomData;

use crate::syntax_node::{
    RevSyntaxNodeChildren, SyntaxElementChildren, SyntaxNode, SyntaxNodeChildren, SyntaxToken,
};
use crate::SyntaxKind;

mod expr_ext;
mod generated;
mod node_ext;
mod operators;
mod token_ext;
mod traits;

pub use self::expr_ext::{ArrayExprKind, LiteralKind};
pub use self::generated::{nodes::*, tokens::*};
pub use self::node_ext::{
    BranchKind, ConstraintKind, ConstraintValue, PathSegment, PathSegmentKind,
};
pub use self::operators::{AssignOp, BinaryOp, UnaryOp};
pub use self::traits::*;

/// The main trait to go from untyped `SyntaxNode` to a typed `AstNode`.
///
/// The conversion itself has zero runtime cost: ast and syntax nodes have exactly
/// the same representation: a pointer to the tree root and a pointer to the
/// node itself.
pub trait AstNode {
    /// Can a `SyntaxNode` with `kind` be cast into this typed `AstNode`?
    fn can_cast(kind: SyntaxKind) -> bool
    where
        Self: Sized;

    /// Cast an untyped `SyntaxNode` into a typed `AstNode` if possible.
    fn cast(syntax: SyntaxNode) -> Option<Self>
    where
        Self: Sized;

    /// Unwrap the typed `AstNode` to get inner untyped `SyntaxNode`.
    fn syntax(&self) -> &SyntaxNode;

    /* JW: not used
    #[must_use]
    fn clone_for_update(&self) -> Self
    where
        Self: Sized,
    {
        Self::cast(self.syntax().clone_for_update()).unwrap()
    }

    #[must_use]
    fn clone_subtree(&self) -> Self
    where
        Self: Sized,
    {
        Self::cast(self.syntax().clone_subtree()).unwrap()
    }
    */
}

/// Like `AstNode`, but wraps tokens rather than interior nodes.
pub trait AstToken {
    fn can_cast(token: SyntaxKind) -> bool
    where
        Self: Sized;

    /// cast a `SyntaxToken` to corresponding `AstToken`
    fn cast(syntax: SyntaxToken) -> Option<Self>
    where
        Self: Sized;

    /// unwrap the `AstNode` to get inner `SynatxNode`
    fn syntax(&self) -> &SyntaxToken;

    // Not used. --Jingwei
    fn text(&self) -> &str {
        self.syntax().text()
    }
}

/// An iterator over `SyntaxNode` children of a particular AST type.
#[derive(Debug, Clone)]
pub struct AstChildren<N> {
    inner: SyntaxNodeChildren,
    ph: PhantomData<N>,
}

impl<N> AstChildren<N> {
    fn new(parent: &SyntaxNode) -> Self {
        AstChildren { inner: parent.children(), ph: PhantomData }
    }
}

impl<N: AstNode> Iterator for AstChildren<N> {
    type Item = N;
    fn next(&mut self) -> Option<N> {
        self.inner.find_map(N::cast)
    }
}

// in reverse order
#[derive(Debug, Clone)]
pub struct RevAstChildren<N> {
    inner: RevSyntaxNodeChildren,
    ph: PhantomData<N>,
}

impl<N> RevAstChildren<N> {
    fn new(parent: &SyntaxNode) -> Self {
        RevAstChildren { inner: RevSyntaxNodeChildren::new(parent), ph: PhantomData }
    }
}

impl<N: AstNode> Iterator for RevAstChildren<N> {
    type Item = N;
    fn next(&mut self) -> Option<N> {
        self.inner.find_map(N::cast)
    }
}

#[derive(Debug, Clone)]
pub struct AstChildTokens<N> {
    inner: SyntaxElementChildren,
    ph: PhantomData<N>,
}

impl<N> AstChildTokens<N> {
    fn new(parent: &SyntaxNode) -> Self {
        AstChildTokens { inner: parent.children_with_tokens(), ph: PhantomData }
    }
}

impl<N: AstToken> Iterator for AstChildTokens<N> {
    type Item = N;
    fn next(&mut self) -> Option<N> {
        self.inner.find_map(|child| N::cast(child.into_token()?))
    }
}

/// Wrappers for `rowan` API
pub(crate) mod support {
    use super::{
        AstChildTokens, AstChildren, AstNode, AstToken, RevAstChildren, SyntaxKind, SyntaxNode,
        SyntaxToken,
    };
    /// the first immediate child node of `parent`
    pub(crate) fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
        parent.children().find_map(N::cast)
    }
    /// the iterator containing all children nodes of `parent`
    pub(crate) fn children<N: AstNode>(parent: &SyntaxNode) -> AstChildren<N> {
        AstChildren::new(parent)
    }
    /// the reverse iterator containing all children nodes of `parent`
    pub(crate) fn rev_children<N: AstNode>(parent: &SyntaxNode) -> RevAstChildren<N> {
        RevAstChildren::new(parent)
    }
    /// the iterator containing both children nodes and tokens of `parent`
    pub(crate) fn child_tokens<N: AstToken>(parent: &SyntaxNode) -> AstChildTokens<N> {
        AstChildTokens::new(parent)
    }
    /// get the child token of `parent` with specified syntax kind
    pub(crate) fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
        parent.children_with_tokens().filter_map(|it| it.into_token()).find(|it| it.kind() == kind)
    }
}
