//! Syntax Tree library
//!
//! The most interesting modules here are `syntax_node` (which defines concrete
//! syntax tree) and `ast` (which defines abstract syntax tree on top of the CST).
//!
//! The actual parser live in a separate `parser` crate.
//! The `parsing` module serves as the bridge and build the syntax tree.
//!
//! See Also:
//! - https://github.com/rust-lang/rust-analyzer/tree/master/crates/syntax
//! - https://docs.rs/ra_ap_syntax/latest/ra_ap_syntax/index.html
//! - https://github.com/rust-lang/rfcs/pull/2256
//! - https://github.com/apple/swift/blob/13d593df6f359d0cb2fc81cfaac273297c539455/lib/Syntax/README.md

pub use preprocessor::diagnostics::PreprocessError;
pub use preprocessor::sourcemap::{self, SourceMap};
pub use preprocessor::{preprocess, Preprocess, SourceProvider};
pub use rowan::{Direction, GreenNode, NodeOrToken, TextRange, TextSize, WalkEvent};
pub use tokens::{SyntaxKind, T};

mod error;
mod parsing;
mod ptr;
mod syntax_node;
mod token_text;
mod validation;

pub mod ast;
pub mod name;

pub use ast::{AstNode, SourceFile};
pub use error::SyntaxError;
pub use parsing::{parse, Parse};
pub use ptr::{AstPtr, SyntaxNodePtr};
pub use syntax_node::{SyntaxNode, SyntaxToken};
use token_text::TokenText;
