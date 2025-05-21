use super::*;
use exprs::arg_list;
use params::{aliasparam_decl, param_decl};
use stmts::{stmt, stmt_with_attrs, STMT_RECOVERY, STMT_TS};
use vars::var_decl;

pub(super) const MODULE_ITEM_RECOVERY: TokenSet = DIRECTION_TS.union(TokenSet::new(&[
    T![net_type],
    T![analog],
    T![initial],
    T![branch],
    T![string],
    T![real],
    T![integer],
    T![parameter],
    T![localparam],
    T![aliasparam],
    T![endmodule],
    EOF,
]));

pub(super) fn module_items(p: &mut Parser) {
    let mut error_range: Option<CompletedMarker> = None;
    while !p.at_ts(ITEM_RECOVERY.union(TokenSet::unique(T![endmodule]))) {
        let m = p.start();
        attrs(p, MODULE_ITEM_RECOVERY);

        match p.current() {
            T![analog] if p.nth(1) == T![function] => func_decl(p, m),
            T![analog] => {
                p.bump(T![analog]);
                p.eat(T![initial]);
                stmt_with_attrs(p);
                m.complete(p, ANALOG_BEHAVIOR);
            }
            T![input] | T![output] | T![inout] => port_decl::<false>(p, m),
            T![net_type] => net_decl::<true>(p, m),
            T![ident] => net_decl::<false>(p, m),
            T![branch] => branch_decl(p, m),
            T![parameter] | T![localparam] => param_decl(p, m),
            T![aliasparam] => aliasparam_decl(p, m),
            T![integer] | T![real] | T![string] => var_decl(p, m),
            _ => {
                error_range = if let Some(error_range) = error_range {
                    m.abandon(p);
                    p.bump_any();
                    while !p.at_ts(MODULE_ITEM_RECOVERY) {
                        p.bump_any();
                    }
                    Some(error_range.undo_completion(p).complete(p, ERROR))
                } else {
                    let err = p.err_with_expected_syntaxes(vec![
                        NET_DECL,
                        BRANCH_DECL,
                        VAR_DECL,
                        PARAM_DECL,
                        FUNCTION,
                        ANALOG_BEHAVIOR,
                    ]);
                    p.error(err);
                    p.bump_any();
                    while !p.at_ts(MODULE_ITEM_RECOVERY) {
                        p.bump_any();
                    }
                    Some(m.complete(p, ERROR))
                }
            }
        }
    }
}

const MODULE_PORTS_RECOVERY: TokenSet = TokenSet::new(&[T![;], T![')'], T![endmodule], EOF]);
const DIRECTION_TS: TokenSet = TokenSet::new(&[T![inout], T![output], T![input]]);

pub(super) fn module_ports(p: &mut Parser) {
    let m = p.start();
    while !p.at_ts(MODULE_PORTS_RECOVERY) {
        let m = p.start();
        if !eat_name(p) {
            let m = p.start();
            attrs(p, MODULE_PORTS_RECOVERY.union(DIRECTION_TS));
            port_decl::<true>(p, m)
        }
        m.complete(p, MODULE_PORT);
        if !p.at(T![')']) {
            p.expect_with(T![,], vec![T![,], T![')']]);
        }
    }
    p.expect(T![')']);
    m.complete(p, MODULE_PORTS);
}

const MODULE_PORT_RECOVERY: TokenSet =
    MODULE_PORTS_RECOVERY.union(DIRECTION_TS).union(TokenSet::unique(T!["(*"]));

// using const generics here for compile-time evaluation and optimization
fn port_decl<const MODULE_HEAD: bool>(p: &mut Parser, m: Marker) {
    // port direction is always required
    let direction = p.start();
    p.bump_ts(DIRECTION_TS);
    direction.complete(p, DIRECTION);

    // discipline_ident and net_type are optional, as either one is required
    if !p.nth_at_ts(1, MODULE_PORT_RECOVERY.union(TokenSet::unique(T![,]))) {
        // discipline ident
        eat_name_ref(p);
    }

    p.eat(T![net_type]);

    if MODULE_HEAD {
        decl_list(p, module_port, T![')'], MODULE_PORT_RECOVERY);
    } else {
        decl_list(p, name, T![;], NET_RECOVERY);
    }
    let finished = m.complete(p, PORT_DECL);
    if !MODULE_HEAD {
        let m = finished.precede(p);
        p.eat(T![;]);
        m.complete(p, BODY_PORT_DECL);
    }

    fn module_port(p: &mut Parser) -> bool {
        name_r(p, MODULE_PORT_RECOVERY.union(TokenSet::unique(T![,])));
        !(p.at(T![,]) && p.nth_at_ts(1, MODULE_PORT_RECOVERY))
    }
}

const NET_RECOVERY: TokenSet = TokenSet::new(&[EOF, T![endmodule]]);

fn net_decl<const NET_TYPE_FIRST: bool>(p: &mut Parser, m: Marker) {
    // discipline_ident and net_type are both optional
    if NET_TYPE_FIRST {
        p.bump(NET_TYPE);
        if !p.nth_at_ts(1, TokenSet::new(&[T![,], T![;]])) {
            eat_name_ref(p);
        }
    } else {
        name_ref_r(p, MODULE_ITEM_OR_ATTR_RECOVERY.union(TokenSet::unique(T![;])))
    }
    decl_list(p, name, T![;], NET_RECOVERY);
    p.eat(T![;]);
    m.complete(p, NET_DECL);
}

fn branch_decl(p: &mut Parser, m: Marker) {
    p.bump(T![branch]);
    if !p.at(T!['(']) {
        p.error(p.err_with_expected_syntax(T!['(']));
    }
    arg_list(p);
    decl_list(p, name, T![;], MODULE_ITEM_OR_ATTR_RECOVERY);
    p.eat(T![;]);
    m.complete(p, BRANCH_DECL);
}

const FUNCTION_RECOVERY: TokenSet = TokenSet::new(&[EOF, T![endmodule], T![endfunction]]);
const FUN_ITEM_TS: TokenSet = TokenSet::new(&[T![parameter], T![localparam]])
    .union(TYPE_TS)
    .union(STMT_RECOVERY)
    .union(DIRECTION_TS)
    .union(STMT_TS);

fn func_decl(p: &mut Parser, m: Marker) {
    p.bump(T![analog]);
    p.bump(T![function]);
    eat_ty(p);
    name_r(p, TokenSet::unique(T![;]));
    p.expect(T![;]);

    while !p.at_ts(FUNCTION_RECOVERY) {
        let m = p.start();
        attrs(p, FUN_ITEM_TS.union(FUNCTION_RECOVERY));
        if p.at_ts(TYPE_TS) {
            var_decl(p, m)
        } else if p.at_ts(TokenSet::new(&[T![parameter], T![localparam]])) {
            param_decl(p, m)
        } else if p.at_ts(DIRECTION_TS) {
            func_arg(p, m);
        } else {
            stmt(p, m, FUN_ITEM_TS, FUNCTION_RECOVERY)
        }
    }
    p.expect(T![endfunction]);
    m.complete(p, FUNCTION);
}

const FUNC_ARG_RECOVERY: TokenSet = TokenSet::new(&[EOF, T![endmodule]]);

fn func_arg(p: &mut Parser, m: Marker) {
    let direction = p.start();
    p.bump_ts(DIRECTION_TS);
    direction.complete(p, DIRECTION);

    decl_list(p, name, T![;], FUNC_ARG_RECOVERY);
    p.eat(T![;]);
    m.complete(p, FUNCTION_ARG);
}
