//! Syntax Tree library
//!
//! The most interesting modules here are `syntax_node` (which defines concrete
//! syntax tree) and `ast` (which defines abstract syntax tree on top of the CST).
//!
//! The actual parser live in a separate `parser` crate.
//! The `parsing` module serves as the bridge and build the syntax tree.
//!
//! See Also:
//! - [`syntax` crate of rust-analyzer](https://docs.rs/ra_ap_syntax/latest/ra_ap_syntax/index.html)
//! - [This RFC](https://github.com/rust-lang/rfcs/pull/2256)
//! - [`libsyntax` of swift](https://github.com/apple/swift/blob/13d593df6f359d0cb2fc81cfaac273297c539455/lib/Syntax/README.md)

pub use preprocessor::errors::PreprocessError;
pub use preprocessor::sourcemap::{self, SourceMap};
pub use preprocessor::{preprocess, Preprocess, SourceProvider};
pub use rowan::{GreenNode, NodeOrToken, TextRange, TextSize, WalkEvent};
pub use tokens::{SyntaxKind, T};

pub mod ast;
pub mod name;

mod parsing;
mod ptr;
mod syntax_error;
mod syntax_node;
mod token_text;
mod validation;

pub use ast::{AstNode, SourceFile};
pub use name::{AsIdent, AsName, Name};
pub use parsing::{parse, Parse};
pub use ptr::{AstPtr, SyntaxNodePtr};
pub use syntax_error::SyntaxError;
pub use syntax_node::{SyntaxNode, SyntaxToken};
