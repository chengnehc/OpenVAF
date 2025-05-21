use super::*;

#[inline]
pub(super) fn name(p: &mut Parser) -> bool {
    name_r(p, TokenSet::new(&[T![,], T![;]]));
    true
}

/// Create a `Name` node, or an error node with `recovery` set.
pub(super) fn name_r(p: &mut Parser, recovery: TokenSet) {
    if p.at(T![ident]) {
        let m = p.start();
        p.bump_any();
        m.complete(p, NAME);
    } else {
        let err = p.err_with_expected_syntax(NAME);
        p.err_recover(err, recovery);
    }
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

/// Try consuming a `Name` token or do nothing if it errs.
pub(super) fn eat_name(p: &mut Parser) -> bool {
    let m = p.start();
    if p.eat(T![ident]) {
        m.complete(p, NAME);
        true
    } else {
        m.abandon(p);
        false
    }
}

pub(super) fn name_ref_r(p: &mut Parser, recovery: TokenSet) {
    let m = p.start();
    if p.eat(T![ident]) {
        m.complete(p, NAME_REF);
    } else {
        m.abandon(p);
        let err = p.err_with_expected_syntax(NAME);
        p.err_recover(err, recovery);
    }
}

pub(super) fn eat_name_ref(p: &mut Parser) -> bool {
    let m = p.start();
    if p.eat(T![ident]) {
        m.complete(p, NAME_REF);
        true
    } else {
        m.abandon(p);
        false
    }
}
