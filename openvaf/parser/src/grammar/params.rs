use super::*;
use items::MODULE_ITEM_OR_ATTR_RECOVERY;

pub(super) fn param_ref(p: &mut Parser) {
    if p.at(T![sysfun]) {
        let m = p.start();
        p.bump_any();
        m.complete(p, SYS_FUN);
    } else {
        name_ref_r(p, TokenSet::unique(T![;]));
    }
}

pub(super) fn param_decl(p: &mut Parser, m: Marker) {
    p.bump_any(); // bump the parameter/localparam keyword
    eat_ty(p);
    decl_list(p, parameter, T![;], MODULE_ITEM_OR_ATTR_RECOVERY);
    p.eat(T![;]);
    m.complete(p, PARAM_DECL);
}

pub(super) fn aliasparam_decl(p: &mut Parser, m: Marker) {
    p.bump(T![aliasparam]);
    name_r(p, TokenSet::new(&[T![;], T![=]]));
    p.expect(T![=]);
    param_ref(p);
    p.eat(T![;]);
    m.complete(p, ALIAS_PARAM);
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
