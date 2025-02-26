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

use tokens::T;

use crate::parser::{CompletedMarker, Marker, Parser};
use crate::SyntaxKind::{self, *};
use crate::TokenSet;

mod attributes;
mod call;
mod expressions;
mod items;
mod paths;
mod stmts;

use attributes::attrs;
use call::arg_list;
use expressions::expr;
use items::ITEM_RECOVERY_SET;
use stmts::{stmt, stmt_with_attrs};

const TYPE_TS: TokenSet = TokenSet::new(&[T![real], T![integer], T![string]]);

pub(crate) fn source_file(p: &mut Parser) {
    let m = p.start();
    let mut error_range: Option<CompletedMarker> = None;
    while !p.at(EOF) {
        let m = p.start();
        attrs(p, ITEM_RECOVERY_SET);
        match p.current() {
            T![discipline] => {
                error_range.take();
                items::discipline(p, m);
            }
            T![nature] => {
                error_range.take();
                items::nature(p, m)
            }
            T![module] => {
                error_range.take();
                items::module(p, m)
            }
            _ => {
                error_range = if let Some(error_range) = error_range {
                    m.abandon(p);
                    p.bump_any();
                    while !p.at_ts(ITEM_RECOVERY_SET) {
                        p.bump_any();
                    }
                    Some(error_range.undo_completion(p).complete(p, ERROR))
                } else {
                    let err =
                        p.err_with_expected_syntaxes(&[T![discipline], T![nature], T![module]]);
                    p.error(err);
                    p.bump_any();
                    while !p.at_ts(ITEM_RECOVERY_SET) {
                        p.bump_any();
                    }
                    Some(m.complete(p, ERROR))
                }
            }
        }
    }
    m.complete(p, SOURCE_FILE);
}

// TODO(JW): does start then abandon undermine performance?
// is testing whether the parser is at some token first more idiomatic?

// start anyway, complete or abandon after
fn ty(p: &mut Parser) {
    let m = p.start();
    if p.expect_ts(TYPE_TS) {
        m.complete(p, TYPE);
    } else {
        m.abandon(p);
    }
}

fn eat_ty(p: &mut Parser) {
    let m = p.start();
    if p.eat_ts(TYPE_TS) {
        m.complete(p, TYPE);
    } else {
        m.abandon(p);
    }
}

// test the current token kind first
fn name_r(p: &mut Parser, recovery: TokenSet) {
    if p.at(T![ident]) {
        let m = p.start();
        p.bump(T![ident]);
        m.complete(p, NAME);
    } else {
        let err = p.err_with_expected_syntax(NAME);
        p.err_recover(err, recovery);
    }
}

fn name(p: &mut Parser) {
    name_r(p, TokenSet::EMPTY);
}

// fn expect_name(p: &mut Parser) -> bool {
//     if eat_name(p) {
//         true
//     } else {
//         let err = p.unexpected_token_msg(NAME);
//         p.error(err);
//         false
//     }
// }

fn eat_name(p: &mut Parser) -> bool {
    let m = p.start();
    if p.eat(T![ident]) {
        m.complete(p, NAME);
        true
    } else {
        m.abandon(p);
        false
    }
}

fn name_ref_r(p: &mut Parser, recovery: TokenSet) {
    let m = p.start();
    if p.eat(T![ident]) {
        m.complete(p, NAME_REF);
    } else {
        m.abandon(p);
        let err = p.err_with_expected_syntax(NAME);
        p.err_recover(err, recovery);
    }
}

fn eat_name_ref(p: &mut Parser) -> bool {
    let m = p.start();
    if p.eat(T![ident]) {
        m.complete(p, NAME_REF);
        true
    } else {
        m.abandon(p);
        false
    }
}
