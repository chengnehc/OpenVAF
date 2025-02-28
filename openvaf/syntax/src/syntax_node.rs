//! This module defines Concrete Syntax Tree (CST) used by OpenVAF
//! by wrapping `rowan`'s API.
//!
//! `rowan` is a crate for generic (language-agnostic) lossless syntax tree
//! used by rust-analyzer.
//!
//! The CST includes comments and whitespace, provides a single node type,
//! `SyntaxNode`, and a basic traversal API (parent, children, siblings).

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum VerilogALanguage {}

/// Teaches `rowan` to convert between two `SyntaxKind` types, allowing for a nicer
/// SyntaxNode API where "kinds" are values from our `SyntaxKind`, instead of plain u16.
impl rowan::Language for VerilogALanguage {
    type Kind = crate::SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        Self::Kind::from(raw.0)
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind.into())
    }
}

pub type SyntaxNode = rowan::SyntaxNode<VerilogALanguage>;
pub type SyntaxToken = rowan::SyntaxToken<VerilogALanguage>;
// pub type SyntaxElement = rowan::SyntaxElement<VerilogALanguage>;
pub type SyntaxNodeChildren = rowan::SyntaxNodeChildren<VerilogALanguage>;
pub type SyntaxElementChildren = rowan::SyntaxElementChildren<VerilogALanguage>;

// Syntax node children iterated in reverse order
// Fairly trivial but sadly lacking from rowan
#[derive(Clone, Debug)]
pub struct RevSyntaxNodeChildren {
    next: Option<SyntaxNode>,
}

impl RevSyntaxNodeChildren {
    pub fn new(parent: &SyntaxNode) -> RevSyntaxNodeChildren {
        RevSyntaxNodeChildren { next: parent.last_child() }
    }
}

impl Iterator for RevSyntaxNodeChildren {
    type Item = SyntaxNode;
    fn next(&mut self) -> Option<SyntaxNode> {
        self.next.take().inspect(|next| {
            self.next = next.prev_sibling();
        })
    }
}
