//! The Verilog-A parser.
//!
//! The `Parser` struct from the [`parser`] module is a cursor into the sequence of tokens.
//! Parsing routines use `Parser` to inspect current state and advance the parsing.
//!
//! The actual parsing happens in the [`grammar`] module.
//!
//! The parser doesn't have access to the *text* of the tokens, and makes decisions based
//! solely on their classification(kind).
//!
//! See Also:
//! - https://docs.rs/ra_ap_parser/0.0.259/ra_ap_parser/
//! - https://github.com/rust-lang/rust-analyzer/tree/master/crates/parser

use stdx::pretty;
use tokens::{SyntaxKind, T};

mod event;
mod grammar;
mod output;
mod parser;
mod token_set;

use event::Event;
use output::Output;
use parser::Parser;
use token_set::TokenSet;

pub use output::Step;

type Token = SyntaxKind;

/// Parse a stream of tokens. Unlike tokens produced by the lexer, the input
/// `tokens` doesn't include trivia (whitespace and comments).
pub fn parse(tokens: &[Token]) -> Output {
    let mut p = Parser::new(tokens);

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

#[derive(Debug, Clone)]
pub enum Error {
    // #[display(fmt = "unexpected token {}; expected {}", "found", "expected")]
    UnexpectedToken { expected: pretty::List<Vec<Token>>, found: Token },
    //
    // #[error("{name} was already declared in this Scope!")]
    // AlreadyDeclaredInThisScope { declaration: Span, other_declaration: Span, name: Box<str> },
    //
    // #[error("Unexpected Token!")]
    // MissingOrUnexpectedToken { expected: Token, expected_at: Span, span: Span },
    //
    // #[error("Reached 'endmodule' while still expecting an 'end' delimiter!")]
    // MismatchedDecimeters { start: Span, end: Span },
    //
    // #[error("Unexpected EOF! Expected {expected}")]
    // UnrecognizedEof { expected: ListFormatter<Vec<String>>, span: Span },
    //
    // ExtraToken { span: Span, token: Token },
    //
    // #[error("Unexpected Token!")]
    // UnexpectedToken { span: Span, ignored: Option<Span> },
}
