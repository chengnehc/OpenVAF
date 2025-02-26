use crate::grammar::paths::path;

use super::*;

mod module;
use module::{module_items, module_ports, MODULE_ITEM_OR_ATTR_RECOVERY};

pub(super) const ITEM_RECOVERY_SET: TokenSet =
    TokenSet::new(&[T![discipline], T![nature], T![module], EOF]);

const DISCIPLINE_RECOVERY_SET: TokenSet =
    ITEM_RECOVERY_SET.union(TokenSet::unique(T![enddiscipline]));

pub(super) fn discipline(p: &mut Parser, m: Marker) {
    p.bump(T![discipline]);
    name_r(p, TokenSet::new(&[T![;]]));
    p.eat(T![;]);
    while !p.at_ts(DISCIPLINE_RECOVERY_SET) {
        let m = p.start();
        path(p);
        p.eat(T![=]);
        expr(p);
        if !p.eat(T![;]) {
            let err = p.err_with_expected_syntax(T![;]);
            p.err_recover(err, DISCIPLINE_RECOVERY_SET.union(TokenSet::unique(T![ident])));
        }
        m.complete(p, DISCIPLINE_ATTR);
    }
    p.expect(T![enddiscipline]);
    m.complete(p, DISCIPLINE_DECL);
}

const NATURE_RECOVERY_SET: TokenSet = ITEM_RECOVERY_SET.union(TokenSet::unique(T![endnature]));

pub(super) fn nature(p: &mut Parser, m: Marker) {
    p.bump(T![nature]);
    name_r(p, TokenSet::new(&[T![;], T![:]]));
    if p.eat(T![:]) {
        name_ref_r(p, TokenSet::unique(T![;]));
    }
    p.eat(T![;]);
    while !p.at_ts(NATURE_RECOVERY_SET) {
        let m = p.start();
        name_r(p, TokenSet::unique(T![=]));
        p.expect(T![=]);
        expr(p);
        if !p.eat(T![;]) {
            let err = p.err_with_expected_syntax(T![;]);
            p.err_recover(err, NATURE_RECOVERY_SET.union(TokenSet::unique(T![ident])));
        }
        m.complete(p, NATURE_ATTR);
    }
    p.expect(T![endnature]);
    m.complete(p, NATURE_DECL);
}

pub(super) fn module(p: &mut Parser, m: Marker) {
    p.bump(T![module]);
    name_r(p, TokenSet::new(&[T!['('], T![;]]));
    if p.at(T!['(']) {
        // module ports are optional
        p.bump(T!['(']);
        module_ports(p);
    }
    p.expect(T![;]);
    module_items(p);
    p.expect(T![endmodule]);
    m.complete(p, MODULE_DECL);
}

pub(super) fn decl_list(
    p: &mut Parser,
    terminator: SyntaxKind,
    mut parse_entry: impl FnMut(&mut Parser) -> bool,
    recovery: TokenSet,
) {
    let recovery = recovery.union(TokenSet::new(&[terminator]));
    if p.at_ts(recovery) {
        p.error(p.err_with_expected_syntax(T![ident]));
    } else {
        while !p.at_ts(recovery) && parse_entry(p) {
            if !p.at(terminator) {
                p.expect_with(T![,], &[T![,], terminator]);
            }
        }
    }
}

fn decl_name(p: &mut Parser) -> bool {
    name_r(p, TokenSet::new(&[T![,], T![;]]));
    true
}

pub(super) fn var_decl(p: &mut Parser, m: Marker) {
    ty(p);
    decl_list(p, T![;], var, MODULE_ITEM_OR_ATTR_RECOVERY);
    p.eat(T![;]);
    m.complete(p, VAR_DECL);
}

fn var(p: &mut Parser) -> bool {
    let m = p.start();
    name_r(p, TokenSet::new(&[T![,], T![=], T![;]]));
    if p.eat(T![=]) {
        expr(p);
    }
    m.complete(p, VAR);
    true
}

pub(super) fn parameter_decl(p: &mut Parser, m: Marker) {
    p.bump_any();
    eat_ty(p);
    decl_list(p, T![;], parameter, MODULE_ITEM_OR_ATTR_RECOVERY);
    p.eat(T![;]);
    m.complete(p, PARAM_DECL);
}

const PARAM_RECOVERY: TokenSet = MODULE_ITEM_OR_ATTR_RECOVERY.union(TokenSet::new(&[T![,], T![;]]));

fn parameter(p: &mut Parser) -> bool {
    let m = p.start();
    name_r(p, TokenSet::new(&[T![,], T![;]]));
    p.expect(T![=]);
    expr(p);
    while !p.at_ts(PARAM_RECOVERY) {
        constraint(p)
    }
    m.complete(p, PARAM);
    true
}

fn constraint(p: &mut Parser) {
    let m = p.start();
    if !p.expect_ts_recover(TokenSet::new(&[T![from], T![exclude]]), PARAM_RECOVERY) {
        m.abandon(p);
        return;
    }
    if p.eat(T!["'{"]) || p.eat(T!['{']) {
        // array range (for string parameters)
        expr(p);
        while p.eat(T![,]) {
            expr(p);
        }
        p.expect(T!['}']);
    } else {
        range_or_expr(p);
    }
    m.complete(p, CONSTRAINT);
}

fn range_or_expr(p: &mut Parser) {
    let m = p.start();

    // while all branches parse an expr they need to eat [/( or nothing first
    #[allow(clippy::branches_sharing_code)]
    if p.eat(T!['(']) {
        expr(p);
        if !p.at(T![:]) {
            p.expect(T![')']);
            m.complete(p, PAREN_EXPR);
            return;
        }
    } else if p.eat(T!['[']) {
        expr(p);
    } else {
        expr(p);
        m.abandon(p);
        return;
    }

    p.expect(T![:]);
    expr(p);
    p.expect_ts(TokenSet::new(&[T![')'], T![']']]));
    m.complete(p, RANGE);
}
