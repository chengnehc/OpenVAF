use hir::{BranchWrite, Case, CaseCond, ContributeKind, ExprId, Node, Stmt, StmtId, Type};
use mir::builder::InstBuilder;
use mir::{Opcode, F_ZERO};

use crate::body::BodyLowerContext;
use crate::{CallBackKind, FlowKind, ParamKind, PlaceKind};

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

    fn lower_contribute(&mut self, is_potential: bool, mut branch: BranchWrite, rhs: ExprId) {
        let mut negate = false;
        if let BranchWrite::Unnamed { hi, lo } = branch {
            // If the branch is unnamed, maybe swap the hi and lo node pair
            // since the reversed form could have already been defined and used.
            let (hi, lo) = self.lower_node_pair(&mut negate, (hi, lo), is_potential);
            branch = BranchWrite::Unnamed { hi, lo }
        };
        // Define a supportive variable indicating whether a contribution branch is a
        // potential source. This variable can help deal with switch branches, which
        // dynamically switches between potential and flow source branch according to
        // run-time parameters.
        self.ctxt.def_place(PlaceKind::IsPotential(branch), is_potential.into());

        // Node collapse hint used by most compact models:
        // 1. LHS is potential source
        // 2. RHS is literal zero
        if is_potential && self.body.get_expr(rhs).is_literal_zero() {
            let (mut hi, mut lo) = branch.node_pair(self.ctxt.db);
            if matches!(branch, BranchWrite::Named(_)) {
                (hi, lo) = self.lower_node_pair(&mut negate, (hi, lo), is_potential);
            }
            self.ctxt.call(CallBackKind::CollapseHint(hi, lo), &[]);
        }

        // Define a place for the complement nature of contribution branch at current basic block.
        // This is supposedly to be used by switch branches.
        self.ctxt.def_place(PlaceKind::Contribute { branch, is_potential: !is_potential }, F_ZERO);

        // Lower the RHS expression
        let rhs = self.lower_expr(rhs);
        if rhs == F_ZERO {
            // if the RHS expression evalutes to 0 then this is a useless contribution,
            // there's no need to build instruction.
            return;
        }

        // Build instructions
        let place = PlaceKind::Contribute { branch, is_potential };
        let old_val = self.ctxt.use_place(place);

        let new_val = if negate {
            // If a negated branch was defined earlier, just make use of it.
            self.ctxt.ins().fsub(old_val, rhs)
        } else if old_val == F_ZERO {
            // If the branch has never received any contribution before,
            // then no need to add any additional instructions
            rhs
        } else {
            // The branch has received some contribution before. Add RHS to the old value.
            self.ctxt.ins().fadd(old_val, rhs)
        };
        self.ctxt.def_place(place, new_val);
    }

    fn lower_node_pair(
        &mut self,
        negate: &mut bool,
        (hi, lo): (Node, Option<Node>),
        is_potential: bool,
    ) -> (Node, Option<Node>) {
        // justify the nodes in case they are global ground
        let hi = self.ctxt.justify_node(hi);
        let lo = lo.and_then(|lo| self.ctxt.justify_node(lo));
        // maybe swap the node pair
        match (hi, lo) {
            (Some(hi), None) => (hi, None),
            (None, Some(lo)) => {
                *negate = true;
                (lo, None)
            }
            (Some(hi), Some(lo)) => {
                let inverted = PlaceKind::Contribute {
                    branch: BranchWrite::Unnamed { hi: lo, lo: Some(hi) },
                    is_potential,
                };
                if self.ctxt.places.contains(&inverted) {
                    // If an inverted branch was defined earlier, just make use of it
                    *negate = true;
                    (lo, Some(hi))
                } else {
                    // TODO(JW): do we really need to define a parameter for contribute lhs?
                    let param = if is_potential {
                        ParamKind::Potential { hi, lo: Some(lo) }
                    } else {
                        ParamKind::Flow(FlowKind::Unnamed { hi, lo: Some(lo) })
                    };
                    self.ctxt.use_param(param);
                    (hi, Some(lo))
                }
            }
            (None, None) => unreachable!(),
        }
    }

    fn lower_loop(&mut self, cond: ExprId, lower_body: impl FnOnce(&mut Self)) {
        let loop_cond = self.ctxt.create_block();
        let loop_body = self.ctxt.create_block();
        let loop_end = self.ctxt.create_block();

        self.ctxt.ins().jump(loop_cond);

        self.ctxt.switch_to_block(loop_cond);
        let cond = self.lower_expr(cond);
        self.ctxt.ins().br_loop(cond, loop_body, loop_end);
        self.ctxt.seal_block(loop_body);
        self.ctxt.seal_block(loop_end);

        self.ctxt.switch_to_block(loop_body);
        lower_body(self);
        self.ctxt.ins().jump(loop_cond);
        self.ctxt.seal_block(loop_cond);

        self.ctxt.switch_to_block(loop_end);
    }

    // TODO disambiguation required
    //
    // 1. Does default case mean that further cases are ignored?
    // -- LRM seems to suggest that no matter where the default case is placed,
    //    all other conditions are tested prior to default.
    //
    // 2. If one case has matched, will all the following cases be ignored,
    //    or still tested?
    // -- No.
    //
    // See Also: [LRM 5.8.3]
    //
    // # Note
    // This impl support short-circuit evaluation.
    fn lower_case(&mut self, discr: ExprId, case_arms: &[Case]) {
        let discr_op = match self.body.expr_type(discr) {
            Type::Bool => Opcode::Beq,
            Type::Integer => Opcode::Ieq,
            Type::Real => Opcode::Feq,
            Type::String => Opcode::Seq,
            Type::Array { .. } => todo!(),
            ty => unreachable!("Invalid type {ty}"),
        };
        let discr = self.lower_expr(discr);
        let end = self.ctxt.create_block();

        for Case { cond, body } in case_arms {
            let CaseCond::Exprs(exprs) = cond else { continue };

            let body_head = self.ctxt.create_block();

            for e in exprs {
                self.ctxt.ensure_sealed();

                let val = self.lower_expr(*e);

                let old_loc = self.ctxt.get_srcloc();
                self.ctxt.set_srcloc(mir::SourceLoc::new(u32::from(*e) as i32 + 1));
                let cond = self.ctxt.ins().binary1(discr_op, val, discr);
                self.ctxt.set_srcloc(old_loc);

                let next_block = self.ctxt.create_block();
                self.ctxt.ins().br(cond, body_head, next_block);
                self.ctxt.switch_to_block(next_block);
            }

            self.ctxt.seal_block(body_head);

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

        self.ctxt.ensure_sealed();
        self.ctxt.ins().jump(end);

        self.ctxt.seal_block(end);
        self.ctxt.switch_to_block(end);
    }
}
