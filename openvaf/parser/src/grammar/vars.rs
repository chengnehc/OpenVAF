use super::*;
use items::MODULE_ITEM_OR_ATTR_RECOVERY;

pub(super) fn var_decl(p: &mut Parser, m: Marker) {
    ty(p);
    decl_list(p, variable, T![;], MODULE_ITEM_OR_ATTR_RECOVERY);
    p.eat(T![;]);
    m.complete(p, VAR_DECL);
}

fn variable(p: &mut Parser) -> bool {
    let m = p.start();
    name_r(p, TokenSet::new(&[T![,], T![=], T![;]]));
    if p.eat(T![=]) {
        expr(p);
    }
    m.complete(p, VAR);
    true
}
