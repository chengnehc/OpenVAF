use super::*;

const EXPR_EXPECTED: &[SyntaxKind] =
    &[T!['('], T!["'{"], SYSFUN, NAME, LITERAL, T![~], T![!], T![+], T![-]];
const EXPR_RECOVERY: TokenSet = TokenSet::new(&[
    T![;],
    T![endmodule],
    T![endfunction],
    T![endnature],
    T![enddiscipline],
    T![endcase],
    T![end],
]);

pub(super) fn expr(p: &mut Parser) -> Option<CompletedMarker> {
    // start from `bp = 1` since `0` is reserved as `NOT_AN_OP`
    expr_bp(p, 1)
}

/// Binding powers of operators for a Pratt parser.
///
/// See:
/// - <https://www.oilshell.org/blog/2016/11/03.html>
/// - <https://matklad.github.io/2020/04/13/simple-but-powerful-pratt-parsing.html>
/// 
/// See [LRM 4.2.2] for a full list of operator precedence
#[rustfmt::skip]
fn current_op(p: &Parser) -> (u8, SyntaxKind) {
    const NOT_AN_OP: (u8, SyntaxKind) = (0, T![@]);
    match p.current() {
        T![?]  => (1,   T![?]),
        
        T![||]  => (2,   T![||]),
        
        T![&&]  => (3,   T![&&]),
        
        T![|]   => (4,   T![|]),
        
        T![^]   => (5,   T![^]),
        
        T![~^]  => (6,   T![~^]),
        T![^~]  => (6,   T![^~]),
        
        T![&]   => (7,   T![&]),
        
        T![==]  => (8,   T![==]),
        T![!=]  => (8,   T![!=]),

        T![>=]  => (9,   T![>=]),
        T![>]   => (9,   T![>]),
        T![<=]  => (9,   T![<=]),
        T![<]   => (9,   T![<]),
        
        T![<<]   => (10,  T![<<]),
        T![>>]   => (10,  T![>>]),

        T![+]    => (11,  T![+]),
        T![-]    => (11,  T![-]),
        
        T![*]    => (12,  T![*]),
        T![/]    => (12,  T![/]),

        T![%]    => (13, T![%]),

        T![**]   => (14, T![**]),

        _        => NOT_AN_OP
    }
}

/// Parses expression with binding power of at least `bp`.
fn expr_bp(p: &mut Parser, bp: u8) -> Option<CompletedMarker> {
    let mut lhs = atom_expr(p)?;
    loop {
        let (op_bp, op) = current_op(p);
        // parse expressions with relatively high binding power,
        // and stop as soon as it finds the newly parsed op is
        // "weaker" (has less binding power) than `bp`
        if op_bp < bp {
            break;
        }

        if op == T![?] {
            let m = lhs.precede(p);
            p.bump(T![?]);
            expr(p);
            p.expect(T![:]);
            expr(p);
            return Some(m.complete(p, SELECT_EXPR));
        }

        let m = lhs.precede(p); // add a preceding node to completed atom expr lhs
        p.bump(op);
        expr_bp(p, op_bp + 1); // treat all operators left associative
        lhs = m.complete(p, BIN_EXPR);
    }
    Some(lhs)
}

fn atom_expr(p: &mut Parser) -> Option<CompletedMarker> {
    let done = match p.current() {
        // Literal
        INT_NUMBER | SI_REAL_NUMBER | STD_REAL_NUMBER | STR_LIT | T![inf] => {
            let m = p.start();
            p.bump_any();
            m.complete(p, LITERAL)
        }
        // ParenExpr
        T!['('] => paren_expr(p),
        // PortFlow
        T![<] => port_flow(p),
        // PrefixExpr
        T![~] | T![!] | T![-] | T![+] => {
            let m = p.start();
            p.bump_any();
            atom_expr(p);
            m.complete(p, PREFIX_EXPR)
        }
        // PathExpr or Call
        T![ident] | T![root] => {
            let cm = path(p)?;
            if p.at(T!('(')) {
                call(p, cm)
            } else {
                let m = cm.precede(p);
                m.complete(p, PATH_EXPR)
            }
        }
        T![sysfun] => sys_fun_call(p),
        // TODO properly implement arrays
        // T!["'{"] => array_expr(p)
        _ => {
            p.err_recover(p.err_with_expected_syntaxes(EXPR_EXPECTED.to_owned()), EXPR_RECOVERY);
            return None;
        }
    };
    Some(done)
}

fn paren_expr(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump(T!['(']);

    // JW: this is for tuple-like expressions: (expr1, expr2, ..)
    // which is not described in LRM, to the best of my knowledge.
    // while !p.at(EOF) && !p.at(T![')']) {
    //     if expr(p).is_none() {
    //         break;
    //     }
    //     if !p.at(T![')']) {
    //         p.expect(T![,]);
    //     }
    // }

    expr(p);
    p.expect(T![')']);
    m.complete(p, PAREN_EXPR)
}

fn port_flow(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    p.bump(T![<]);
    path(p);
    p.expect(T![>]);
    m.complete(p, PORT_FLOW)
}

fn call(p: &mut Parser, lhs: CompletedMarker) -> CompletedMarker {
    let m = lhs.precede(p);
    arg_list(p);
    m.complete(p, CALL)
}

fn sys_fun_call(p: &mut Parser) -> CompletedMarker {
    let m = p.start();
    let m2 = p.start();
    p.bump(T![sysfun]);
    m2.complete(p, SYS_FUN);
    if p.at(T!('(')) {
        arg_list(p);
    }
    m.complete(p, CALL)
}

pub(super) fn arg_list(p: &mut Parser) {
    let m = p.start();
    p.eat(T!['(']);
    while !p.at(T![')']) && !p.at(EOF) {
        if expr(p).is_none() {
            break;
        }
        if !p.at(T![')']) && !p.expect(T![,]) {
            break;
        }
    }
    p.eat(T![')']);
    m.complete(p, ARG_LIST);
}

// fn array_expr(p: &mut Parser) -> CompletedMarker {
//     let m = p.start();
//     p.bump(T!["'{"]);
//     while !p.at(EOF) && !p.at(T![']']) {
//         // test array_attrs
//         // const A: &[i64] = &[1, #[cfg(test)] 2];
//         if expr(p).is_none() {
//             break;
//         }

//         if !p.at(T!['}']) && !p.expect(T![,]) {
//             break;
//         }
//     }
//     p.expect(T!['}']);

//     m.complete(p, ARRAY_EXPR)
// }
