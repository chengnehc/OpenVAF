//! Various extension methods to ast Nodes, which are hard to code-generate.

use std::borrow::Cow;
use std::iter::successors;

use rowan::{GreenNodeData, GreenTokenData, NodeOrToken};

use crate::ast::{self, support, ArgListOwner, AstChildren, AstNode};
use crate::SyntaxKind::{IDENT, ROOT_KW};
use crate::{SyntaxNode, SyntaxToken, TokenText};

impl ast::Name {
    pub fn text(&self) -> TokenText<'_> {
        text_of_first_token(self.syntax())
    }
}

impl ast::NameRef {
    pub fn text(&self) -> TokenText<'_> {
        text_of_first_token(self.syntax())
    }
}

fn text_of_first_token(node: &SyntaxNode) -> TokenText<'_> {
    fn first_token(green_ref: &GreenNodeData) -> &GreenTokenData {
        green_ref.children().next().and_then(NodeOrToken::into_token).unwrap()
    }

    match node.green() {
        Cow::Borrowed(green_ref) => TokenText::borrowed(first_token(green_ref).text()),
        Cow::Owned(green) => TokenText::owned(first_token(&green).to_owned()),
    }
}

impl ast::Path {
    #[must_use]
    pub fn first_qualifier(&self) -> ast::Path {
        successors(Some(self.clone()), ast::Path::qualifier).last().unwrap()
    }

    pub fn qualifiers(&self) -> impl Iterator<Item = ast::Path> + Clone {
        successors(self.qualifier(), |p| p.qualifier())
    }

    // pub fn first_segment(&self) -> Option<PathSegment> {
    //    self.first_qualifier().segment()
    // }

    pub fn parent(&self) -> Option<ast::Path> {
        self.syntax().parent().and_then(ast::Path::cast)
    }

    /// The ultimate parent node that stores the token of the last segment.
    #[must_use]
    pub fn top_path(&self) -> ast::Path {
        successors(Some(self.clone()), ast::Path::parent).last().unwrap()
    }

    // pub fn last_segment(&self) -> Option<ast::PathSegment> {
    //     self.top_path().segment()
    // }

    /// Return the segment that this `Path` node stores.
    pub fn segment(&self) -> Option<PathSegment> {
        self.syntax().children_with_tokens().find_map(|e| {
            let kind = match e.kind() {
                IDENT => PathSegmentKind::Name,
                ROOT_KW => PathSegmentKind::Root,
                _ => return None,
            };
            Some(PathSegment { kind, token: e.into_token().unwrap() })
        })
    }

    pub fn segment_token(&self) -> Option<SyntaxToken> {
        self.syntax()
            .children_with_tokens()
            .find(|e| matches!(e.kind(), IDENT | ROOT_KW))
            .and_then(|e| e.into_token())
    }

    pub fn segment_kind(&self) -> Option<PathSegmentKind> {
        self.syntax().children_with_tokens().find_map(|e| match e.kind() {
            IDENT => Some(PathSegmentKind::Name),
            ROOT_KW => Some(PathSegmentKind::Root),
            _ => None,
        })
    }

    // pub fn segments(&self) -> impl Iterator<Item = PathSegment> + Clone {
    //    successors(self.first_segment(), |p| {
    //        p.parent().and_then(|path| path.parent()).and_then(|p| p.segment())
    //    })
    // }

    pub fn as_raw_ident(&self) -> Option<SyntaxToken> {
        let segment = self.segment()?;
        let is_valid =
            self.qualifier().is_none() && matches!(segment.kind, ast::PathSegmentKind::Name);
        is_valid.then_some(segment.token)
    }
}

#[derive(PartialEq, Eq, Debug, Clone)]
pub struct PathSegment {
    pub token: SyntaxToken,
    pub kind: PathSegmentKind,
}

#[derive(PartialEq, Eq, Debug, Clone, Copy)]
pub enum PathSegmentKind {
    Root,
    Name,
}

impl ast::ModuleDecl {
    pub fn analog_behaviors(&self) -> impl Iterator<Item = ast::Stmt> {
        support::children::<ast::AnalogBehavior>(self.syntax())
            .filter(|it| it.initial_token().is_none())
            .filter_map(|it| it.stmt())
    }

    pub fn analog_initial_behaviors(&self) -> impl Iterator<Item = ast::Stmt> {
        support::children::<ast::AnalogBehavior>(self.syntax())
            .filter(|it| it.initial_token().is_some())
            .filter_map(|it| it.stmt())
    }

    pub fn body_ports(&self) -> AstChildren<ast::BodyPortDecl> {
        support::children(self.syntax())
    }
}

impl ast::ModulePorts {
    pub fn decls(&self) -> AstChildren<ast::PortDecl> {
        support::children(self.syntax())
    }

    pub fn names(&self) -> AstChildren<ast::Name> {
        support::children(self.syntax())
    }
}

impl ast::Function {
    pub fn body(&self) -> AstChildren<ast::Stmt> {
        support::children(self.syntax())
    }

    pub fn args(&self) -> AstChildren<ast::FunctionArg> {
        support::children(self.syntax())
    }
}

impl ast::BranchDecl {
    pub fn branch_kind(&self) -> Option<BranchKind> {
        let nodes = self.arg_list()?;
        let node1 = nodes.args().next()?;
        let node2 = nodes.args().nth(1);
        let kind = match node2 {
            Some(node2) => BranchKind::Nodes(node1.as_path()?, node2.as_path()?),
            None => {
                if let Some(node) = node1.as_path() {
                    BranchKind::NodeGnd(node)
                } else if let ast::Expr::PortFlow(port_flow) = node1 {
                    BranchKind::PortFlow(port_flow)
                } else {
                    return None;
                }
            }
        };

        Some(kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchKind {
    PortFlow(ast::PortFlow),
    NodeGnd(ast::Path),
    Nodes(ast::Path, ast::Path),
}

impl ast::Constraint {
    pub fn kind(&self) -> Option<ConstraintKind> {
        if self.from_token().is_some() {
            Some(ConstraintKind::Include)
        } else if self.exclude_token().is_some() {
            Some(ConstraintKind::Exclude)
        } else {
            None
        }
    }

    pub fn val(&self) -> Option<ConstraintValue> {
        if let Some(range) = self.range() {
            Some(ConstraintValue::Range(range))
        } else {
            Some(ConstraintValue::Value(self.expr()?))
        }
    }
}

#[derive(Debug, Eq, PartialEq, Clone, Copy)]
pub enum ConstraintKind {
    Include,
    Exclude,
}

#[derive(Debug, Eq, PartialEq)]
pub enum ConstraintValue {
    Range(ast::Range),
    Value(ast::Expr),
}

impl ast::Range {
    pub fn lower_inclusive(&self) -> bool {
        self.l_brack_token().is_some()
    }

    pub fn upper_inclusive(&self) -> bool {
        self.r_brack_token().is_some()
    }

    pub fn lower_bound(&self) -> Option<ast::Expr> {
        support::children(self.syntax()).next()
    }

    pub fn upper_bound(&self) -> Option<ast::Expr> {
        support::children(self.syntax()).nth(1)
    }
}
