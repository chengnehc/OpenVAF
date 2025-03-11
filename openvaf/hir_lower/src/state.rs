use hir::CompilationDB;
use lasso::Rodeo;
use mir::Function;
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

        let mut ctxt = MainLowerContext::new(db, builder, true, self);
        for (kind, param) in ctxt.intern.params.clone().iter() {
            if let ParamKind::HiddenState(var) = *kind {
                if ctxt.dfg().value_dead(*param) {
                    continue;
                }
                let val = ctxt.lower_expr_body(var.init(db).borrow(), 0);
                ctxt.dfg_mut().replace_uses(*param, val);
            }
        }
        ctxt.ensured_sealed();
        ctxt.func.func.layout.append_inst_to_bb(term, ctxt.current_block())
    }
}
