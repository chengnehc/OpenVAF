use std::iter::zip;

use hir::BodyRef;
use mir::builder::InstBuilder;
use mir::{Block, Value};

use crate::ctx::MainLowerContext;

pub struct BodyLowerContext<'a, 'c1, 'c2> {
    pub ctxt: &'a mut MainLowerContext<'c1, 'c2>,
    pub body: BodyRef<'a>,
    pub path: &'a str,
}

impl<'a> BodyLowerContext<'a, '_, '_> {
    pub fn with_body(mut self, body: BodyRef<'a>) -> Self {
        self.body = body;
        self
    }
}

impl<'c1, 'c2> BodyLowerContext<'_, 'c1, 'c2> {
    pub fn lower_entry_stmts(&mut self) {
        for &stmt in self.body.entry_stmts() {
            self.lower_stmt(stmt)
        }
    }
    pub fn lower_select(
        &mut self,
        cond: Value,
        mut lower_then_expr: impl FnMut(BodyLowerContext<'_, 'c1, 'c2>) -> Value,
        mut lower_else_expr: impl FnMut(BodyLowerContext<'_, 'c1, 'c2>) -> Value,
    ) -> Value {
        self.ctxt.make_select_expr(cond, |ctxt, br| {
            let body_ctxt = BodyLowerContext { ctxt, body: self.body, path: self.path };
            if br {
                lower_then_expr(body_ctxt)
            } else {
                lower_else_expr(body_ctxt)
            }
        })
    }

    pub fn lower_cond<T>(
        &mut self,
        cond: Value,
        mut lower_body: impl FnMut(BodyLowerContext<'_, 'c1, 'c2>, bool) -> T,
    ) -> ((Block, T), (Block, T)) {
        self.ctxt.make_if_stmt(cond, |ctxt, br| {
            let body_ctxt = BodyLowerContext { ctxt, body: self.body, path: self.path };
            lower_body(body_ctxt, br)
        })
    }

    pub fn lower_multi_select<const N: usize>(
        &mut self,
        cond: Value,
        lower_body: impl FnMut(BodyLowerContext<'_, 'c1, 'c2>, bool) -> [Value; N],
    ) -> [Value; N] {
        let ((then_bb, mut then_vals), (else_bb, else_vals)) = self.lower_cond(cond, lower_body);
        for (then_val, else_val) in zip(&mut then_vals, else_vals) {
            *then_val = self.ctxt.ins().phi1(&[(then_bb, *then_val), (else_bb, else_val)]);
        }
        then_vals
    }
}

impl MainLowerContext<'_, '_> {
    pub fn lower_expr_body(&mut self, body: BodyRef, i: usize) -> Value {
        BodyLowerContext { ctxt: self, body, path: "" }.lower_expr(body.get_nth_entry_expr(i))
    }
}
