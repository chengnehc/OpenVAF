use std::marker::PhantomData;

use crate::syntax_node::{RevSyntaxNodeChildren, SyntaxElementChildren, SyntaxNodeChildren};
use crate::{SyntaxKind, SyntaxNode, SyntaxToken};

mod generated;
pub use generated::{nodes::*, tokens::*};

mod expr_ext;
mod node_ext;
mod token_ext;

mod operators;
mod traits;

pub use expr_ext::{ArrayExprKind, LiteralKind};
pub use node_ext::{BranchKind, ConstraintKind, ConstraintValue, PathSegment, PathSegmentKind};
pub use operators::{AssignOp, BinaryOp, UnaryOp};
pub use traits::*;

/// Matches a `SyntaxNode` against a typed `AstNode`.
///
/// # Example:
///
/// ```ignore
/// match_ast! {
///     match node {
///         ast::CallExpr(it) => { ... },
///         ast::MethodCallExpr(it) => { ... },
///         ast::MacroCall(it) => { ... },
///         _ => None,
///     }
/// }
/// ```
#[macro_export]
macro_rules! match_ast {
    (match $node:ident { $($tt:tt)* }) => { match_ast!(match ($node) { $($tt)* }) };

    (match ($node:expr) {
        $( ast::$ast:ident($it:ident) => $res:expr, )*
        _ => $catch_all:expr $(,)?
    }) => {{
        $( if let Some($it) = ast::$ast::cast($node.clone()) { $res } else )*
        { $catch_all }
    }};
}

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
}

/// Like `AstNode`, but wraps tokens rather than interior nodes.
pub trait AstToken {
    fn can_cast(token: SyntaxKind) -> bool
    where
        Self: Sized;

    /// cast a `SyntaxToken` to an `AstToken`, if possible
    fn cast(syntax: SyntaxToken) -> Option<Self>
    where
        Self: Sized;

    /// unwrap the `AstNode` to get inner `SynatxNode`
    fn syntax(&self) -> &SyntaxToken;

    /// return the text string of this token
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
    /// Return the first immediate child node typed `N` of `parent`.
    pub(crate) fn child<N: AstNode>(parent: &SyntaxNode) -> Option<N> {
        parent.children().find_map(N::cast)
    }
    /// Iterate through children nodes typed `N` of `parent`.
    pub(crate) fn children<N: AstNode>(parent: &SyntaxNode) -> AstChildren<N> {
        AstChildren::new(parent)
    }
    /// Iterate reversely through children nodes typed `N` of `parent`.
    pub(crate) fn rev_children<N: AstNode>(parent: &SyntaxNode) -> RevAstChildren<N> {
        RevAstChildren::new(parent)
    }
    /// Iterate through child token nodes typed `N` of `parent`.
    pub(crate) fn child_tokens<N: AstToken>(parent: &SyntaxNode) -> AstChildTokens<N> {
        AstChildTokens::new(parent)
    }
    /// Get the child token of `parent` according to specified syntax `kind`.
    pub(crate) fn token(parent: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
        parent.children_with_tokens().filter_map(|it| it.into_token()).find(|it| it.kind() == kind)
    }
}
