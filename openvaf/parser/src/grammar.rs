//! This is the actual "grammar" of the VerilogA language.
//!
//! Each function in this module and its children corresponds
//! to a production of the formal grammar. Submodules roughly
//! correspond to different *areas* of the grammar. By convention,
//! each submodule starts with `use super::*` import and exports
//! "public" productions via `pub(super)`.
//!
//! See docs for [`Parser`](super::parser::Parser) to learn about API
//! available to the grammar, and see docs for [`Event`](super::event::Event)
//! to learn how this actually manages to produce parse trees.

use crate::parser::{CompletedMarker, Marker, Parser};
use crate::SyntaxKind::{self, *};
use crate::{TokenSet, T};

mod attrs;
mod exprs;
mod items;
mod names;
mod params;
mod paths;
mod stmts;
mod types;
mod vars;

pub(crate) use items::source_file;
use names::*;
use types::*;

use attrs::attrs;
use exprs::expr;
use paths::path;

/// Parse a list of declarations.
fn decl_list(
    p: &mut Parser,
    mut parse_entry: impl FnMut(&mut Parser) -> bool,
    terminator: SyntaxKind,
    recovery: TokenSet,
) {
    let recovery = recovery.union(TokenSet::unique(terminator));
    if !p.at_ts(recovery) {
        while !p.at_ts(recovery) && parse_entry(p) {
            if !p.at(terminator) {
                p.expect_with(T![,], vec![T![,], terminator]);
            }
        }
    } else {
        p.error(p.err_with_expected_syntax(T![ident]));
    }
}
