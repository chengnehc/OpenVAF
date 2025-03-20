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
            Stmt::Contribute { kind, lhs, rhs } => {
                self.lower_contribute(kind == ContributeKind::Potential, lhs, rhs)
            }
            Stmt::If { cond, then_stmt, else_stmt } => {
                let cond = self.lower_expr(cond);
                self.lower_cond(cond, |mut cx, br| {
                    let stmt = if br { then_stmt } else { else_stmt };
                    cx.lower_stmt(stmt)
                });
            }
            Stmt::ForLoop { init, cond, incr, body } => {
                self.lower_stmt(init);
                self.lower_loop(cond, |cx| {
                    cx.lower_stmt(body);
                    cx.lower_stmt(incr);
                });
            }
            Stmt::WhileLoop { cond, body } => self.lower_loop(cond, |cx| cx.lower_stmt(body)),
            Stmt::Case { discr, case_arms } => self.lower_case(discr, case_arms),
            Stmt::EventControl { body, .. } => self.lower_stmt(body), // TODO handle properly
        }
    }

    fn lower_contribute(&mut self, is_potential: bool, mut lhs: BranchWrite, rhs: ExprId) {
        let mut negate = false;
        if let BranchWrite::Unnamed { hi, lo } = &mut lhs {
            // maybe swap hi and lo node here
            self.lower_contribute_node_pair(&mut negate, hi, lo, is_potential)
        }
        self.ctxt.def_place(PlaceKind::IsPotential(lhs), is_potential.into());

        // Node collapse hint used by most compact models:
        // 1. LHS is potential access
        // 2. RHS is literal zero
        if is_potential && self.body.get_expr(rhs).is_literal_zero() {
            let (mut hi, mut lo) = lhs.node_pair(self.ctxt.db);
            if matches!(lhs, BranchWrite::Named(_)) {
                self.lower_contribute_node_pair(&mut negate, &mut hi, &mut lo, is_potential)
            }
            self.ctxt.call(CallBackKind::CollapseHint(hi, lo), &[]);
        }

        // JW: I guess this is meant to reserve a place for the complement nature access
        // of branch write dst. If it is already declared, then this is a no-op.
        self.ctxt.def_place(
            PlaceKind::Contribute { dst: lhs, is_reactive: false, is_potential: !is_potential },
            F_ZERO,
        );

        let rhs = self.lower_expr(rhs);

        // no need to build instruction if the RHS expression evalutes to 0
        if rhs == F_ZERO {
            return;
        }

        let place = PlaceKind::Contribute { dst: lhs, is_reactive: false, is_potential };
        let old_val = self.ctxt.use_place(place);
        let new_val = if negate {
            self.ctxt.ins().fsub(old_val, rhs)
        } else if old_val == F_ZERO {
            rhs
        } else {
            self.ctxt.ins().fadd(old_val, rhs)
        };
        self.ctxt.def_place(place, new_val);
    }

    fn lower_contribute_node_pair(
        &mut self,
        negate: &mut bool,
        hi: &mut Node,
        lo: &mut Option<Node>,
        is_potential: bool,
    ) {
        // justify the nodes in case they are global ground
        let hi_ = self.ctxt.justify_node(*hi);
        let lo_ = lo.and_then(|lo| self.ctxt.justify_node(lo));
        // maybe swap the node pair
        (*hi, *lo) = match (hi_, lo_) {
            (Some(hi), None) => (hi, None),
            (None, Some(lo)) => {
                *negate = true;
                (lo, None)
            }
            (Some(hi), Some(lo)) => {
                let negate_place_def = PlaceKind::Contribute {
                    dst: BranchWrite::Unnamed { hi: lo, lo: Some(hi) },
                    is_reactive: false,
                    is_potential,
                };
                if self.ctxt.places.contains(&negate_place_def) {
                    // If a negated form of contribute branch was defined earlier,
                    // just make use of it
                    *negate = true;
                    (lo, Some(hi))
                } else {
                    // If not, define a new param
                    let param_kind = if is_potential {
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
