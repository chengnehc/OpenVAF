//! Hidden state: a variable without proper initialization could
// cause hidden state, which should not be used by compact models.

use hir::CompilationDB;
use mir::{Function, Rodeo};
use mir_build::{FunctionBuilder, FunctionBuilderContext};

use crate::ctx::MainLowerContext;
use crate::{HirInterner, ParamKind};

impl HirInterner {
    pub fn insert_var_init(
        &mut self,
        db: &CompilationDB,
        func: &mut Function,
        literals: &mut Rodeo,
    ) {
        let mut func_ctxt = FunctionBuilderContext::default();
        let (builder, term) = FunctionBuilder::edit(func, literals, &mut func_ctxt, false);

        // temporarily take ownership to work around borrow checker
        let params = std::mem::take(&mut self.params);
        let mut ctxt = MainLowerContext::new(db, builder, self);
        for (kind, param) in params.iter() {
            if let ParamKind::HiddenState(var) = *kind {
                if !ctxt.dfg().value_dead(*param) {
                    let val = ctxt.lower_first_stmt_expr(var.body(db).borrow());
                    ctxt.dfg_mut().replace_uses(*param, val);
                }
            }
        }
        ctxt.intern.params = params;
        ctxt.ensure_sealed();
        ctxt.func.func.layout.append_inst_to_block(term, ctxt.current_block())
    }
}
