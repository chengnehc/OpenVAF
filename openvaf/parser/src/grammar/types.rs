use super::*;

pub(super) const TYPE_TS: TokenSet = TokenSet::new(&[T![real], T![integer], T![string]]);

/// Create a `Type` (integer, real, string) node or an error node.
pub(super) fn ty(p: &mut Parser) {
    let m = p.start();
    if p.expect_ts(TYPE_TS) {
        m.complete(p, TYPE);
    } else {
        m.abandon(p);
    }
}

/// Try consuming a `Type` token or do nothing if it errs.
pub(super) fn eat_ty(p: &mut Parser) {
    let m = p.start();
    if p.eat_ts(TYPE_TS) {
        m.complete(p, TYPE);
    } else {
        m.abandon(p);
    }
}
