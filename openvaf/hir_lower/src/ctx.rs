//! The main HIR lowering context.

use ahash::AHashSet;
use hir::{CompilationDB, Node, Type, Variable};
use mir::builder::{InsertBuilder, InstBuilder};
use mir::{Block, DataFlowGraph, FuncRef, Ieee64, Inst, Opcode, SourceLoc, Value};
use mir::{FALSE, F_ZERO, INFINITY, TRUE};
use mir_build::{FuncInstBuilder, FunctionBuilder, Place};
use typed_indexmap::TiSet;

use crate::{
    CallBackKind, HirInterner, ImplicitEquation, ImplicitEquationKind, LimitState, ParamKind,
    PlaceKind,
};

pub struct MainLowerContext<'a, 'c> {
    pub db: &'a CompilationDB,
    pub func: FunctionBuilder<'c>,
    pub intern: &'a mut HirInterner,

    pub places: TiSet<Place, PlaceKind>,
    tagged_vars: AHashSet<Variable>,

    pub no_equations: bool,
    pub inside_lim: bool,
    /// We create a dedicated callback for each noise source
    /// by giving each callback a unique index. Kind of ineffcient
    /// but necessary to avoid accidental correlation/opimization.
    /// For example white_noise(x) - white_noise(x) is not zero.
    pub num_noise_sources: u32,
}

impl<'a, 'c> MainLowerContext<'a, 'c> {
    pub fn new(
        db: &'a CompilationDB,
        func: FunctionBuilder<'c>,
        no_equations: bool,
        intern: &'a mut HirInterner,
    ) -> Self {
        Self {
            db,
            func,
            intern,
            no_equations,
            places: TiSet::default(),
            tagged_vars: AHashSet::default(),
            inside_lim: false,
            num_noise_sources: 0,
        }
    }

    /// A builder method to include tagged variables.
    pub fn with_tagged_reads(mut self, vars: AHashSet<Variable>) -> Self {
        self.tagged_vars = vars;
        self
    }

    /// Return the MIR SSA value of a HIR `Variable`.
    ///
    /// This function should be used to correctly handle tagging.
    pub fn read_var(&mut self, var: Variable) -> Value {
        let mut val = self.use_place(PlaceKind::Var(var));
        if self.tagged_vars.contains(&var) {
            val = self.func.ins().optbarrier(val);
            self.intern.tagged_reads.insert(val, var);
        }
        val
    }

    /// Declare and initialize (if necessary) a mutable memory slot in the entry block,
    /// which will be translated to SSA values automatically.
    ///
    /// If the requested kind of place already exists, simply return it, otherwise a
    /// new memory slot is created.
    pub fn dec_place(&mut self, kind: PlaceKind) -> Place {
        use PlaceKind::*;
        let (place, new) = self.places.ensure(kind);
        if new {
            // initialize the place
            let init_val = match kind {
                // such kinds of places are always initialized
                Param(_)
                | ParamMin(_)
                | ParamMax(_)
                | FunctionReturn { .. }
                | FunctionArg { .. } => return place,

                // such kinds of places require initialization
                // TODO(JW) hidden state: a variable without proper initialization could
                // cause hidden state, which should not be used by compact models.
                Var(var) => self.use_param(ParamKind::HiddenState(var)),
                Contribute { .. } | ImplicitResidual { .. } => F_ZERO,
                CollapseImplicitEquation(_) => TRUE,
                IsPotential(_) => FALSE,
                BoundStep => INFINITY,
            };
            let block = self.func.func.layout.entry_block().unwrap();
            self.func.def_var_at(place, init_val, block);
        }
        place
    }

    /// Declare a place and define it with `val` at current basic block.
    /// SSA value numbering algorithm is applied.
    pub fn def_place(&mut self, kind: PlaceKind, val: Value) {
        let place = self.dec_place(kind);
        self.func.def_var(place, val)
    }

    /// Ensure the place is initialized and use it at current basic block.
    /// SSA value numbering algorithm is applied.
    pub fn use_place(&mut self, kind: PlaceKind) -> Value {
        let place = self.dec_place(kind);
        self.func.use_var(place)
    }

    /// Get the SSA value of the parameter kind. If it does not exist in the
    /// interner, insert a new one and make a related SSA value.
    pub fn use_param(&mut self, kind: ParamKind) -> Value {
        let len = self.intern.params.len();
        *self.intern.params.raw.entry(kind).or_insert_with(|| self.func.make_param(len.into()))
    }

    /// Define a parameter with `val`. This will overwrite the previous value.
    pub fn def_param(&mut self, kind: ParamKind, val: Value) {
        self.intern.params.insert(kind, val);
    }

    /// Define a output with `val`. This will overwrite the previous value.
    pub fn def_output(&mut self, kind: PlaceKind, val: Value) {
        self.intern.outputs.insert(kind, val.into());
    }

    /// Declare a callback function. If it already exists then simply return
    /// the reference to the declared callback.
    pub fn dec_callback(&mut self, kind: CallBackKind) -> FuncRef {
        let sig = kind.signature();
        let (func_ref, changed) = self.intern.callbacks.ensure(kind);
        if changed {
            self.intern.callback_callers.push(Vec::new());
            let sig = self.func.import_function(sig);
            debug_assert_eq!(func_ref, sig);
        }
        func_ref
    }

    /// Make an instruction that calls a callback function of `kind` with `args`
    pub fn call(&mut self, kind: CallBackKind, args: &[Value]) -> Inst {
        let tracked = !self.no_equations && kind.tracked();
        let func_ref = self.dec_callback(kind);
        let (inst, _) = self.ins().call(func_ref, args);
        if tracked {
            self.intern.callback_callers[func_ref].push(inst)
        }
        inst
    }

    /// Make an instruction that calls a callback function of `kind` with `args`
    /// and return its first result value
    pub fn call1(&mut self, kind: CallBackKind, args: &[Value]) -> Value {
        let inst = self.call(kind, args);
        self.dfg().first_result(inst)
    }

    pub fn justify_node(&self, node: Node) -> Option<Node> {
        if node.is_gnd(self.db) {
            None
        } else {
            Some(node)
        }
    }

    pub fn node_pair(
        &mut self,
        hi: Node,
        lo: Option<Node>,
        kind: impl Fn(Node, Option<Node>) -> ParamKind,
    ) -> Value {
        let hi = self.justify_node(hi);
        let lo = lo.and_then(|lo| self.justify_node(lo));
        match (hi, lo) {
            (Some(hi), None) => self.use_param(kind(hi, None)),
            (None, Some(lo)) => {
                let lo = self.use_param(kind(lo, None));
                self.func.ins().fneg(lo)
            }
            (Some(hi), Some(lo)) => {
                if let Some(inverted) = self.intern.params.get(&kind(lo, Some(hi))) {
                    self.func.ins().fneg(*inverted)
                } else {
                    self.use_param(kind(hi, Some(lo)))
                }
            }
            (None, None) => F_ZERO,
        }
    }

    /// Create an implicit equation with `kind`, and define
    /// - a flag variable to indicate whether the equation is collapsible
    /// - the function parameter corresponding to the implicit unknown
    pub fn implicit_equation(&mut self, kind: ImplicitEquationKind) -> (ImplicitEquation, Value) {
        let equation = self.intern.implicit_equations.push_and_get_key(kind);
        let place = self.dec_place(PlaceKind::CollapseImplicitEquation(equation));
        self.func.def_var(place, FALSE);
        // self.def_place(PlaceKind::CollapseImplicitEquation(equation), FALSE);
        let val = self.use_param(ParamKind::ImplicitUnknown(equation));
        (equation, val)
    }

    pub fn def_implicit_residual(
        &mut self,
        val: Value,
        equation: ImplicitEquation,
        reactive: bool,
    ) {
        let place = PlaceKind::ImplicitResidual { equation, reactive };
        let place = self.dec_place(place);
        self.func.def_var(place, val);
    }

    /// Start lowering a `$limit` function by allocating a state slot
    /// for the limit call. `probe` is the first argument (voltage or current probe)
    /// to `$limit`.
    ///
    /// The returned limit state *must* be passed to `finish_limit` to ensure corectness
    pub fn start_limit(&mut self, probe: Value) -> LimitState {
        let mut unknown = probe;
        if let Some(inst) = self.dfg().value_def(unknown).inst() {
            debug_assert_eq!(self.dfg().insts[inst].opcode(), Opcode::Fneg);
            unknown = self.dfg().instr_args(inst)[0];
        }
        let dst = self.intern.lim_state.raw.entry(unknown);
        let state = LimitState::from(dst.index());
        // value is a placeholder that will be populated by insert_limit
        dst.or_default().push((F_ZERO, probe != unknown));
        debug_assert!(!self.inside_lim);
        self.inside_lim = true;
        state
    }

    pub fn finish_limit(&mut self, state: LimitState, mut val: Value) -> Value {
        val = self.call1(CallBackKind::StoreLimit(state), &[val]);
        self.intern.lim_state[state].last_mut().unwrap().0 = val;
        debug_assert!(self.inside_lim);
        self.inside_lim = false;
        val
    }

    pub fn make_type_cast(&mut self, val: Value, src: &Type, dst: &Type) -> Value {
        let op = match (dst, src) {
            (Type::Real, Type::Integer) => Opcode::IFcast,
            (Type::Integer, Type::Real) => Opcode::FIcast,
            (Type::Bool, Type::Real) => Opcode::FBcast,
            (Type::Real, Type::Bool) => Opcode::BFcast,
            (Type::Integer, Type::Bool) => Opcode::BIcast,
            (Type::Bool, Type::Integer) => Opcode::IBcast,

            (Type::Array { .. }, Type::EmptyArray) | (Type::EmptyArray, Type::Array { .. }) => {
                return val
            }
            _ => unreachable!("unknown cast found {src:?} -> {dst:?}"),
        };

        self.ins().unary1(op, val)
    }

    /// `cond ? then_expr : else_expr`  <=>  `if cond { then_expr } else { else_expr }`
    pub fn make_select_expr(
        &mut self,
        cond: Value,
        lower_branch: impl FnMut(&mut Self, bool) -> Value,
    ) -> Value {
        let (then_src, else_src) = self.make_if_stmt(cond, lower_branch);
        self.ins().phi1(&[then_src, else_src])
    }

    pub fn make_if_stmt<T>(
        &mut self,
        cond: Value,
        mut lower_branch: impl FnMut(&mut Self, bool) -> T,
    ) -> ((Block, T), (Block, T)) {
        let then_dst = self.create_block();
        let else_dst = self.create_block();
        let merge_bb = self.create_block();

        self.ins().br(cond, then_dst, else_dst);
        self.seal_block(then_dst);
        self.seal_block(else_dst);

        self.switch_to_block(then_dst);
        let then_val = lower_branch(self, true);
        let then_tail = self.current_block();
        self.ins().jump(merge_bb);

        self.switch_to_block(else_dst);
        let else_val = lower_branch(self, false);
        let else_tail = self.current_block();
        self.ins().jump(merge_bb);

        self.switch_to_block(merge_bb);
        self.ensure_sealed();

        ((then_tail, then_val), (else_tail, else_val))
    }
}

impl<'c> MainLowerContext<'_, 'c> {
    #[inline]
    pub(crate) fn dfg(&self) -> &DataFlowGraph {
        &self.func.func.dfg
    }
    #[inline]
    pub(crate) fn dfg_mut(&mut self) -> &mut DataFlowGraph {
        &mut self.func.func.dfg
    }
    #[inline]
    pub(crate) fn get_srcloc(&self) -> SourceLoc {
        self.func.get_srcloc()
    }
    #[inline]
    pub(crate) fn set_srcloc(&mut self, loc: SourceLoc) {
        self.func.set_srcloc(loc)
    }
    #[inline]
    pub(crate) fn current_block(&self) -> Block {
        self.func.current_block()
    }
    #[inline]
    pub(crate) fn create_block(&mut self) -> Block {
        self.func.create_block()
    }
    #[inline]
    pub(crate) fn switch_to_block(&mut self, block: Block) {
        self.func.switch_to_block(block)
    }
    #[inline]
    pub(crate) fn seal_block(&mut self, block: Block) {
        self.func.seal_block(block)
    }
    #[inline]
    pub(crate) fn ensure_sealed(&mut self) {
        self.func.ensure_sealed()
    }
    #[inline]
    pub(crate) fn ins(&mut self) -> InsertBuilder<'_, FuncInstBuilder<'_, 'c>> {
        self.func.ins()
    }
    #[inline]
    pub(crate) fn fconst(&mut self, val: Ieee64) -> Value {
        self.func.fconst(val)
    }
    #[inline]
    pub(crate) fn iconst(&mut self, val: i32) -> Value {
        self.func.iconst(val)
    }
    #[inline]
    pub(crate) fn sconst(&mut self, val: &str) -> Value {
        self.func.sconst(val)
    }
}
