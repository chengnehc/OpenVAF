//! OpenVAF MIR parser
//!
//! See also the `cranelift-reader` crate:
//! - https://docs.rs/cranelift-reader/latest/cranelift_reader/index.html

mod error;
mod lexer;
mod parser;

pub use error::{ParseError, ParseResult};
pub use lexer::LexError;
pub use parser::{parse_function, parse_functions};
