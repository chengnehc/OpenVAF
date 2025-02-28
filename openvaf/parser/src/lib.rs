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

pub(crate) use tokens::{SyntaxKind, T};

mod error;
mod event;
mod grammar;
mod output;
mod parser;
mod token_set;

pub use error::SyntaxError;
pub use output::{Output, Step};
pub(crate) use token_set::TokenSet;

/// The parser doesn't have access to the *text* of the tokens, and makes
/// decisions based solely on their classification(kind).
///
/// Unlike tokens produced by the lexer, the input `tokens` doesn't include
/// trivia (whitespace and comments).
pub fn parse(tokens: &[SyntaxKind]) -> Output {
    let mut p = parser::Parser::new(tokens);

    // source file is the only entry point of the parser
    grammar::source_file(&mut p);
    let events = p.finish();
    let output = event::process(events);

    if cfg!(debug_assertions) {
        let mut depth = 0;
        let mut first = true;
        for step in output.iter() {
            assert!(depth > 0 || first);
            first = false;
            match step {
                Step::Enter { .. } => depth += 1,
                Step::Exit => depth -= 1,
                Step::Token { .. } | Step::Error { .. } => (),
            }
        }
        assert!(!first, "no tree at all");
        assert_eq!(depth, 0, "unbalanced tree");
    }

    output
}

/* JW: not used.
pub struct Error {
    pub expected: pretty::List<Vec<Token>>,
    pub found: Token,
}
*/
