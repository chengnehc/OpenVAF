use hir::{BranchWrite, Case, CaseCond, ContributeKind, ExprId, Node, Stmt, StmtId, Type};
use mir::builder::InstBuilder;
use mir::{Opcode, F_ZERO};

use crate::body::BodyLowerContext;
use crate::{CallBackKind, CurrentKind, ParamKind, PlaceKind};

impl BodyLowerContext<'_, '_, '_> {
    pub(super) fn lower_stmt(&mut self, stmt: StmtId) {
        let Some(stmt) = self.body.get_stmt(stmt) else { return };
        match stmt {
            Stmt::Expr(expr) => {
                self.lower_expr(expr);
            }
            Stmt::Block { body } => body.iter().for_each(|&stmt| self.lower_stmt(stmt)),
            Stmt::Assignment { lhs, rhs } => {
                let val = self.lower_expr(rhs);
                self.ctxt.def_place(lhs.into(), val);
            }
            Stmt::Contribute { kind, branch, rhs } => {
                self.lower_contribute(kind == ContributeKind::Potential, branch, rhs)
            }
            Stmt::If { cond, then_branch, else_branch } => {
                let cond = self.lower_expr(cond);
                self.ctxt.make_if(cond, |ctxt, br| {
                    let stmt = if br { then_branch } else { else_branch };
                    BodyLowerContext { ctxt, body: self.body, path: self.path }.lower_stmt(stmt);
                });
            }
            Stmt::ForLoop { init, cond, incr, body } => {
                self.lower_stmt(init);
                self.lower_loop(cond, |s| {
                    s.lower_stmt(body);
                    s.lower_stmt(incr);
                });
            }
            Stmt::WhileLoop { cond, body } => self.lower_loop(cond, |s| s.lower_stmt(body)),
            Stmt::Case { discr, case_arms } => self.lower_case(discr, case_arms),
            Stmt::EventControl { body, .. } => self.lower_stmt(body), // TODO handle properly
        }
    }

    fn lower_contribute(&mut self, is_potential: bool, mut dst: BranchWrite, rhs: ExprId) {
        let mut negate = false;
        if let BranchWrite::Unnamed { hi, lo } = &mut dst {
            self.lower_contribute_unnamed_branch(&mut negate, hi, lo, is_potential)
        }
        self.ctxt.def_place(PlaceKind::IsPotential(dst), is_potential.into());

        // node collapse:
        // - `V(n1, n2) <+ 0`
        // - `V(b1) <+ 0`
        let (mut hi, mut lo) = dst.nodes(self.ctxt.db);
        let is_zero = self.body.get_expr(rhs).is_zero();
        if is_potential && is_zero {
            if matches!(dst, BranchWrite::Named(_)) {
                self.lower_contribute_unnamed_branch(&mut negate, &mut hi, &mut lo, is_potential)
            }
            self.ctxt.call(CallBackKind::CollapseHint(hi, lo), &[]);
        }

        self.ctxt.def_place(
            PlaceKind::Contribute { dst, is_reactive: false, is_potential: !is_potential },
            F_ZERO,
        );
        let rhs = self.lower_expr(rhs);
        if rhs == F_ZERO {
            return;
        }

        let place = PlaceKind::Contribute { dst, is_reactive: false, is_potential };
        let old = self.ctxt.use_place(place);
        let new = if negate {
            self.ctxt.ins().fsub(old, rhs)
        } else if old == F_ZERO {
            rhs
        } else {
            self.ctxt.ins().fadd(old, rhs)
        };
        self.ctxt.def_place(place, new);
    }

    fn lower_contribute_unnamed_branch(
        &mut self,
        negate: &mut bool,
        hi: &mut Node,
        lo: &mut Option<Node>,
        potential: bool,
    ) {
        let hi_ = self.ctxt.node(*hi);
        let lo_ = lo.and_then(|lo| self.ctxt.node(lo));
        (*hi, *lo) = match (hi_, lo_) {
            (Some(hi), None) => (hi, None),
            (None, Some(lo)) => {
                *negate = true;
                (lo, None)
            }
            (Some(hi), Some(lo)) => {
                let kind = PlaceKind::Contribute {
                    dst: BranchWrite::Unnamed { hi: lo, lo: Some(hi) },
                    is_reactive: false,
                    is_potential: potential,
                };
                let negate_known = self.ctxt.get_place(kind).is_some();
                if negate_known {
                    *negate = true;
                    (lo, Some(hi))
                } else {
                    let param_kind = if potential {
                        ParamKind::Voltage { hi, lo: Some(lo) }
                    } else {
                        ParamKind::Current(CurrentKind::Unnamed { hi, lo: Some(lo) })
                    };
                    self.ctxt.use_param(param_kind);
                    (hi, Some(lo))
                }
            }
            (None, None) => unreachable!(),
        };
    }

    fn lower_loop(&mut self, cond: ExprId, lower_body: impl FnOnce(&mut Self)) {
        let loop_cond_head = self.ctxt.create_block();
        let loop_body_head = self.ctxt.create_block();
        let loop_end = self.ctxt.create_block();

        self.ctxt.ins().jump(loop_cond_head);
        self.ctxt.switch_to_block(loop_cond_head);

        let cond = self.lower_expr(cond);
        self.ctxt.ins().br_loop(cond, loop_body_head, loop_end);
        self.ctxt.seal_block(loop_body_head);
        self.ctxt.seal_block(loop_end);

        self.ctxt.switch_to_block(loop_body_head);
        lower_body(self);
        self.ctxt.ins().jump(loop_cond_head);

        self.ctxt.seal_block(loop_cond_head);

        self.ctxt.switch_to_block(loop_end);
    }

    fn lower_case(&mut self, discr: ExprId, case_arms: &[Case]) {
        let discr_op = match self.body.expr_type(discr) {
            Type::Bool => Opcode::Beq,
            Type::Integer => Opcode::Ieq,
            Type::Real => Opcode::Feq,
            Type::String => Opcode::Seq,
            Type::Array { .. } => todo!(),
            ty => unreachable!("Invalid type {}", ty),
        };
        let discr = self.lower_expr(discr);
        let end = self.ctxt.create_block();

        for Case { cond, body } in case_arms {
            // TODO does default mean that further cases are ignored?
            // standard seems to suggest that no matter where the default case is placed that all
            // other conditions are tested prior
            let vals = match cond {
                CaseCond::Vals(vals) => vals,
                CaseCond::Default => continue,
            };

            // Create the body block
            let body_head = self.ctxt.create_block();

            for val in vals {
                self.ctxt.ensured_sealed();

                // Lower the condition (val == discriminant)
                let val_ = self.lower_expr(*val);

                let old_loc = self.ctxt.get_srcloc();
                self.ctxt.set_srcloc(mir::SourceLoc::new(u32::from(*val) as i32 + 1));
                let cond = self.ctxt.ins().binary1(discr_op, val_, discr);
                self.ctxt.set_srcloc(old_loc);

                // Create the next block
                let next_block = self.ctxt.create_block();
                self.ctxt.ins().branch(cond, body_head, next_block, false);

                self.ctxt.switch_to_block(next_block);
            }

            self.ctxt.seal_block(body_head);

            // lower the body
            let next = self.ctxt.current_block();
            self.ctxt.switch_to_block(body_head);
            self.lower_stmt(*body);
            self.ctxt.ins().jump(end);
            self.ctxt.switch_to_block(next);
        }

        if let Some(default_case) =
            case_arms.iter().find(|arm| matches!(arm.cond, CaseCond::Default))
        {
            self.lower_stmt(default_case.body);
        }

        self.ctxt.ensured_sealed();
        self.ctxt.ins().jump(end);

        self.ctxt.seal_block(end);
        self.ctxt.switch_to_block(end);
    }
}
