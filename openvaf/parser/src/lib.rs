//! The Verilog-A parser.
//!
//! The `Parser` struct from the [`parser`] module is a cursor into the sequence of tokens.
//! Parsing routines use `Parser` to inspect current state and advance the parsing.
//!
//! The actual parsing happens in the [`grammar`] module.
//!
//! See Also:
//! - https://docs.rs/ra_ap_parser/0.0.259/ra_ap_parser/
//! - https://github.com/rust-lang/rust-analyzer/tree/master/crates/parser

pub(crate) use tokens::SyntaxKind;

mod error;
mod event;
mod grammar;
mod output;
mod parser;
mod token_set;

pub use error::SyntaxError;
pub use output::{Output, Step};
pub(crate) use token_set::TokenSet;

pub fn parse(tokens: &[SyntaxKind]) -> Output {
    let mut p = parser::Parser::new(tokens);
    grammar::source_file(&mut p);
    let events = p.finish();
    event::process(events)
}

/* JW: not used.
pub struct Error {
    pub expected: pretty::List<Vec<Token>>,
    pub found: Token,
}
*/
