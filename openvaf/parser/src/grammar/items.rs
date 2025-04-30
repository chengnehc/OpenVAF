//! Top level items of Verilog-A source file.

use super::*;

mod module;

use module::MODULE_ITEM_RECOVERY;
use module::{module_items, module_ports};

pub(super) const ITEM_RECOVERY: TokenSet =
    TokenSet::new(&[T![discipline], T![nature], T![module], EOF]);

/// The entry function of parser.
pub(crate) fn source_file(p: &mut Parser) {
    let m = p.start();
    let mut error_range: Option<CompletedMarker> = None;
    while !p.at(EOF) {
        let m = p.start();
        attrs(p, ITEM_RECOVERY);
        match p.current() {
            T![nature] => {
                error_range.take();
                items::nature(p, m)
            }
            T![discipline] => {
                error_range.take();
                items::discipline(p, m);
            }
            T![module] => {
                error_range.take();
                items::module(p, m)
            }
            _ => {
                error_range = if let Some(error_range) = error_range {
                    m.abandon(p);
                    p.bump_any();
                    while !p.at_ts(ITEM_RECOVERY) {
                        p.bump_any();
                    }
                    Some(error_range.undo_completion(p).complete(p, ERROR))
                } else {
                    let err =
                        p.err_with_expected_syntaxes(vec![T![discipline], T![nature], T![module]]);
                    p.error(err);
                    p.bump_any();
                    while !p.at_ts(ITEM_RECOVERY) {
                        p.bump_any();
                    }
                    Some(m.complete(p, ERROR))
                }
            }
        }
    }
    m.complete(p, SOURCE_FILE);
}

const NATURE_RECOVERY: TokenSet = ITEM_RECOVERY.union(TokenSet::unique(T![endnature]));

fn nature(p: &mut Parser, m: Marker) {
    p.bump(T![nature]);
    name_r(p, TokenSet::new(&[T![;], T![:]]));
    if p.eat(T![:]) {
        name_ref_r(p, TokenSet::unique(T![;]));
    }
    p.eat(T![;]);
    while !p.at_ts(NATURE_RECOVERY) {
        let m = p.start();
        name_r(p, TokenSet::unique(T![=]));
        p.expect(T![=]);
        expr(p);
        if !p.eat(T![;]) {
            let err = p.err_with_expected_syntax(T![;]);
            p.err_recover(err, NATURE_RECOVERY.union(TokenSet::unique(T![ident])));
        }
        m.complete(p, NATURE_ATTR);
    }
    p.expect(T![endnature]);
    m.complete(p, NATURE_DECL);
}

const DISCIPLINE_RECOVERY: TokenSet = ITEM_RECOVERY.union(TokenSet::unique(T![enddiscipline]));

fn discipline(p: &mut Parser, m: Marker) {
    p.bump(T![discipline]);
    name_r(p, TokenSet::unique(T![;]));
    p.eat(T![;]);
    while !p.at_ts(DISCIPLINE_RECOVERY) {
        let m = p.start();
        path(p);
        p.eat(T![=]);
        expr(p);
        if !p.eat(T![;]) {
            let err = p.err_with_expected_syntax(T![;]);
            p.err_recover(err, DISCIPLINE_RECOVERY.union(TokenSet::unique(T![ident])));
        }
        // p.expect_recover(T![;], DISCIPLINE_RECOVERY.union(TokenSet::unique(T![ident])));
        m.complete(p, DISCIPLINE_ATTR);
    }
    p.expect(T![enddiscipline]);
    m.complete(p, DISCIPLINE_DECL);
}

pub(super) const MODULE_ITEM_OR_ATTR_RECOVERY: TokenSet =
    MODULE_ITEM_RECOVERY.union(TokenSet::unique(T!["(*"]));

fn module(p: &mut Parser, m: Marker) {
    p.bump(T![module]);
    name_r(p, TokenSet::new(&[T!['('], T![;]]));
    if p.eat(T!['(']) {
        module_ports(p);
    }
    p.expect(T![;]);
    module_items(p);
    p.expect(T![endmodule]);
    m.complete(p, MODULE_DECL);
}
