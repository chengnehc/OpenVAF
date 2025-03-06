use crate::grammar::items::{param_decl, var_decl};

use super::*;

pub(super) const STMT_TS: TokenSet = TokenSet::new(&[
    T![if],
    T![while],
    T![for],
    T![case],
    T![begin],
    T![;],
    T![ident],
    T![sysfun],
    T![@],
]);
pub(super) const STMT_RECOVERY: TokenSet = TokenSet::new(&[EOF, T![endmodule], T![;]]);
pub(super) const STMT_ATTR_RECOVERY: TokenSet =
    STMT_RECOVERY.union(TokenSet::new(&[T![if], T![while], T![for], T![case], T![begin]]));

pub(super) fn stmt_with_attrs(p: &mut Parser) {
    let m = p.start();
    attrs(p, STMT_ATTR_RECOVERY);
    stmt(p, m, STMT_TS, STMT_RECOVERY)
}

pub(super) fn stmt(p: &mut Parser, m: Marker, expected: TokenSet, recovery: TokenSet) {
    match p.current() {
        T![;] => empty_stmt(p, m),
        T![ident] | T![sysfun] => expr_or_assign_stmt::<true>(p, m),
        T![begin] => block_stmt(p, m),
        T![if] => if_stmt(p, m),
        T![while] => while_stmt(p, m),
        T![for] => for_stmt(p, m),
        T![case] => case_stmt(p, m),
        T![@] => event_stmt(p, m),
        _ => {
            m.abandon(p);
            let err = p.err_with_expected_syntaxes(expected.iter().collect());
            p.err_recover(err, recovery);
        }
    }
}

fn empty_stmt(p: &mut Parser, m: Marker) {
    p.bump(T![;]);
    m.complete(p, EMPTY_STMT);
}

fn expr_or_assign_stmt<const SEMICOLON: bool>(p: &mut Parser, m: Marker) {
    let kind = if eat_assign(p) { ASSIGN_STMT } else { EXPR_STMT };
    if SEMICOLON {
        p.expect(T![;]);
    }
    m.complete(p, kind);
}

fn eat_assign(p: &mut Parser) -> bool {
    let m = p.start();
    expr(p);
    if p.eat_ts(TokenSet::new(&[T![<+], T![=]])) {
        expr(p);
        m.complete(p, ASSIGN);
        true
    } else {
        m.abandon(p);
        false
    }
}

const PARAM_TS: TokenSet = TokenSet::new(&[T![parameter], T![localparam]]);
const BLOCK_EXPECTED: TokenSet = STMT_TS.union(TYPE_TS).union(PARAM_TS);
const BLOCK_RECOVERY: TokenSet = TokenSet::new(&[EOF, T![end], T![endmodule]]);
const BLOCK_ATTR_RECOVERY: TokenSet =
    BLOCK_RECOVERY.union(STMT_ATTR_RECOVERY).union(TYPE_TS).union(PARAM_TS);

fn block_stmt(p: &mut Parser, m: Marker) {
    p.bump(T![begin]);
    if p.at(T![:]) {
        let m = p.start();
        p.bump(T![:]);
        name(p);
        m.complete(p, BLOCK_SCOPE);
    }
    while !p.at_ts(BLOCK_RECOVERY) {
        let m = p.start();
        attrs(p, BLOCK_ATTR_RECOVERY);
        if p.at_ts(TYPE_TS) {
            var_decl(p, m);
        } else if p.at_ts(PARAM_TS) {
            param_decl(p, m);
        } else {
            stmt(p, m, BLOCK_EXPECTED, BLOCK_RECOVERY)
        }
    }
    p.expect(T![end]);
    m.complete(p, BLOCK_STMT);
}

fn if_stmt(p: &mut Parser, m: Marker) {
    p.bump(T![if]);
    p.expect(T!['(']);
    expr(p);
    p.expect(T![')']);
    stmt_with_attrs(p);
    if p.eat(T![else]) {
        stmt_with_attrs(p)
    }
    m.complete(p, IF_STMT);
}

fn while_stmt(p: &mut Parser, m: Marker) {
    p.bump(T![while]);
    p.expect(T!['(']);
    expr(p);
    p.expect(T![')']);
    stmt_with_attrs(p);
    m.complete(p, WHILE_STMT);
}

fn for_stmt(p: &mut Parser, m: Marker) {
    p.bump(T![for]);
    p.expect(T!['(']);

    // init
    let stmt = p.start();
    attrs(p, STMT_RECOVERY.union(TokenSet::new(&[T![ident]])));
    expr_or_assign_stmt::<true>(p, stmt);

    // cond
    expr(p);
    p.expect(T![;]);

    // incr
    let stmt = p.start();
    attrs(p, STMT_RECOVERY.union(TokenSet::new(&[T![ident]])));
    expr_or_assign_stmt::<false>(p, stmt);

    p.expect(T![')']);
    stmt_with_attrs(p);
    m.complete(p, FOR_STMT);
}

const CASE_ITEM_RECOVERY: TokenSet = TokenSet::new(&[EOF, T![endcase], T![endmodule]]);
const CASE_COND_RECOVERY: TokenSet = TokenSet::new(&[EOF, T![:], T![endcase], T![endmodule]]);
fn case_stmt(p: &mut Parser, m: Marker) {
    p.bump(T![case]);
    p.expect(T!['(']);
    expr(p);
    p.expect(T![')']);

    while !p.at_ts(CASE_ITEM_RECOVERY) {
        case_item(p)
    }

    p.expect(ENDCASE_KW);
    m.complete(p, CASE_STMT);
}

fn case_item(p: &mut Parser) {
    let m = p.start();
    vals_or_default(p);
    stmt_with_attrs(p);
    m.complete(p, CASE);
}

fn vals_or_default(p: &mut Parser) {
    if p.eat(T![default]) {
        p.eat(T![:]);
    } else {
        while !p.at_ts(CASE_COND_RECOVERY) {
            expr(p);
            if !p.at(T![:]) {
                p.expect_with(T![,], vec![T![:], T![,]]);
            }
        }
        p.expect(T![:]);
    }
}

fn event_stmt(p: &mut Parser, m: Marker) {
    p.bump(T![@]);
    p.expect(T!['(']);
    p.expect_ts_recover(
        TokenSet::new(&[T![initial_step], T![final_step]]),
        TokenSet::new(&[T![')'], T!['(']]),
    );
    if p.eat(T!['(']) {
        while !p.at_ts(TokenSet::new(&[T![')'], T![begin], T![endmodule]])) {
            let mut succ = p.expect(STR_LIT);
            if !p.at(T![')']) {
                succ |= p.expect_with(T![,], vec![T![')'], T![,]]);
                if !succ {
                    p.bump_any()
                }
            }
        }
        p.eat(T![')']);
    }
    p.expect(T![')']);
    stmt_with_attrs(p);
    m.complete(p, EVENT_STMT);
}
