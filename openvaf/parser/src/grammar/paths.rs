use super::*;

pub(super) const PATH_SEGMENT_TS: TokenSet = TokenSet::new(&[T![ident], T![root]]);

pub(super) fn path(p: &mut Parser) -> Option<CompletedMarker> {
    let m = p.start();
    let mut qual = qualifier(p, m)?;
    while p.at(T![.]) {
        let m = qual.precede(p);
        p.bump(T![.]);
        qual = qualifier(p, m)?;
    }
    Some(qual)
}

fn qualifier(p: &mut Parser, m: Marker) -> Option<CompletedMarker> {
    if p.eat_ts(PATH_SEGMENT_TS) {
        let qual = m.complete(p, PATH);
        Some(qual)
    } else {
        m.abandon(p);
        let err = p.err_with_expected_syntaxes(vec![T![ident], T![root]]);
        p.err_recover(err, TokenSet::new(&[T!['('], T![;]]));
        None
    }
}
