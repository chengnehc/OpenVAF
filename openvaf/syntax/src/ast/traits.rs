//! Super traits `ArgListOwner` and `AttrsOwner` over `AstNode`

use std::iter::FlatMap;

use crate::ast::{self, support, AstNode, RevAstChildren};
use crate::SyntaxNode;

pub trait ArgListOwner: AstNode {
    fn arg_list(&self) -> Option<ast::ArgList> {
        support::child(self.syntax())
    }
}

pub type AttrIter = FlatMap<
    RevAstChildren<ast::AttrList>,
    RevAstChildren<ast::Attr>,
    fn(ast::AttrList) -> RevAstChildren<ast::Attr>,
>;

pub fn attrs(syntax: &SyntaxNode) -> AttrIter {
    support::rev_children::<ast::AttrList>(syntax)
        .flat_map(|list| support::rev_children::<ast::Attr>(list.syntax()))
}

pub trait AttrsOwner: AstNode {
    fn attrs(&self) -> AttrIter {
        attrs(self.syntax())
    }
    fn has_attr(&self, name: &str) -> bool {
        self.attrs().any(|attr| attr.name().is_some_and(|n| n.text() == name))
    }
    fn get_attr(&self, name: &str) -> Option<ast::Attr> {
        self.attrs().find(|attr| attr.name().is_some_and(|n| n.text() == name))
    }
}
