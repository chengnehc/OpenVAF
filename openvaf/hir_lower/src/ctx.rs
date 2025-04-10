//! The main HIR lowering context.
//!
//! The most important methods are:
//! * def_place
//! * use_place, use_param
//! * call

use ahash::AHashSet;
use hir::{CompilationDB, Node, Type, Variable};
use mir::builder::{InsertBuilder, InstBuilder};
use mir::{
    Block, DataFlowGraph, FuncRef, Ieee64, Inst, Opcode, Param, SourceLoc, Value, FALSE, F_ZERO,
    INFINITY, TRUE,
};
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

    pub no_equations: bool, // This flag means do not lower equations
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

    pub fn with_tagged_vars(mut self, vars: AHashSet<Variable>) -> Self {
        self.tagged_vars = vars;
        self
    }

    /// Declare and initialize (if necessary) a mutable memory location in
    /// the entry block, which will be translated to SSA values automatically.
    ///
    /// If the requested place already exists, simply return it, otherwise a
    /// new memory slot is created.
    pub fn dec_place(&mut self, kind: PlaceKind) -> Place {
        use PlaceKind::*;
        let (place, inserted) = self.places.ensure(kind);
        if inserted {
            let init_val = match kind {
                // such kinds of places are always initialized
                Param(_)
                | ParamMin(_)
                | ParamMax(_)
                | FunctionReturn { .. }
                | FunctionArg { .. } => return place,
                // A `Variable` without proper initialization would cause hidden state
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

    pub fn def_place(&mut self, kind: PlaceKind, val: Value) {
        let place = self.dec_place(kind);
        self.func.def_var(place, val)
    }

    pub fn use_place(&mut self, kind: PlaceKind) -> Value {
        let place = self.dec_place(kind);
        self.func.use_var(place)
    }

    /// Return the MIR SSA value of a HIR variable.
    ///
    /// This function should be used to correctly handle tagging.
    pub fn read_var(&mut self, var: Variable) -> Value {
        let place = self.dec_place(PlaceKind::Var(var));
        let mut val = self.func.use_var(place);
        if self.tagged_vars.contains(&var) {
            val = self.func.ins().optbarrier(val);
            self.intern.tagged_reads.insert(val, var);
        }
        val
    }

    pub fn get_param_value(&self, kind: ParamKind) -> Option<Value> {
        self.intern.params.get(&kind).copied()
    }

    pub fn get_param_kind(&mut self, param: Param) -> &ParamKind {
        let (kind, _) = self.intern.params.get_index(param).unwrap();
        kind
    }

    pub fn def_param(&mut self, kind: ParamKind, val: Value) {
        self.intern.params.insert(kind, val);
    }

    pub fn use_param(&mut self, kind: ParamKind) -> Value {
        let len = self.intern.params.len();
        let entry = self.intern.params.raw.entry(kind);
        *entry.or_insert_with(|| self.func.make_param(len.into()))
    }

    pub fn def_output(&mut self, kind: PlaceKind, val: Value) {
        self.intern.outputs.insert(kind, val.into());
    }

    /// Declare a callback function. If it already exists then simply return
    /// the reference to the declared callback.
    pub fn dec_callback(&mut self, kind: CallBackKind) -> FuncRef {
        let data = kind.signature();
        let (func_ref, changed) = self.intern.callbacks.ensure(kind);
        if changed {
            self.intern.callback_users.push(Vec::new());
            let sig = self.func.import_function(data);
            debug_assert_eq!(func_ref, sig);
        }
        func_ref
    }

    /// Make an instruction that calls a callback function of `kind` with `args`
    pub fn call(&mut self, kind: CallBackKind, args: &[Value]) -> Inst {
        let tracked = !self.no_equations && kind.tracked();
        let func_ref = self.dec_callback(kind);
        let (inst, _) = self.func.ins().call(func_ref, args);
        if tracked {
            self.intern.callback_users[func_ref].push(inst)
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

    pub fn nodes(
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
                if let Some(inverted) = self.get_param_value(kind(lo, Some(hi))) {
                    self.func.ins().fneg(inverted)
                } else {
                    self.use_param(kind(hi, Some(lo)))
                }
            }
            (None, None) => F_ZERO,
        }
    }

    /// Start lowering a `$limit` function by allocating a state slot
    /// for the limit call. `probe` is the first argument (voltage or current probe)
    /// to `$limit`.
    ///
    /// The returned limit state *must* be passed to `finish_limit` to ensure corectness
    pub fn start_limit(&mut self, probe: Value) -> LimitState {
        let mut unknown = probe;
        if let Some(inst) = self.func.func.dfg.value_def(unknown).inst() {
            debug_assert_eq!(self.func.func.dfg.insts[inst].opcode(), Opcode::Fneg);
            unknown = self.func.func.dfg.instr_args(inst)[0];
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

    pub fn implicit_equation(&mut self, kind: ImplicitEquationKind) -> (ImplicitEquation, Value) {
        let equation = self.intern.implicit_equations.push_and_get_key(kind);
        let place = self.dec_place(PlaceKind::CollapseImplicitEquation(equation));
        self.func.def_var(place, FALSE);
        let val = self.use_param(ParamKind::ImplicitUnknown(equation));
        (equation, val)
    }

    pub fn def_resist_residual(&mut self, residual_val: Value, equation: ImplicitEquation) {
        let place = PlaceKind::ImplicitResidual { equation, reactive: false };
        let place = self.dec_place(place);
        self.func.def_var(place, residual_val);
    }

    pub fn def_react_residual(&mut self, residual_val: Value, equation: ImplicitEquation) {
        let place = PlaceKind::ImplicitResidual { equation, reactive: true };
        let place = self.dec_place(place);
        self.func.def_var(place, residual_val);
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
            _ => unreachable!("unknown cast found {:?} -> {:?}", src, dst),
        };

        self.func.ins().unary1(op, val)
    }

    pub fn make_select_expr(
        &mut self,
        cond: Value,
        lower_branch: impl FnMut(&mut Self, bool) -> Value,
    ) -> Value {
        let (then_src, else_src) = self.make_if_stmt(cond, lower_branch);
        self.func.ins().phi1(&[then_src, else_src])
    }

    pub fn make_if_stmt<T>(
        &mut self,
        cond: Value,
        mut lower_branch: impl FnMut(&mut Self, bool) -> T,
    ) -> ((Block, T), (Block, T)) {
        let then_dst = self.func.create_block();
        let else_dst = self.func.create_block();
        let next_bb = self.func.create_block();

        self.func.ins().br(cond, then_dst, else_dst);
        self.func.seal_block(then_dst);
        self.func.seal_block(else_dst);

        self.func.switch_to_block(then_dst);
        self.func.ensure_inserted_block();
        let then_val = lower_branch(self, true);
        self.func.ins().jump(next_bb);
        let then_tail = self.func.current_block();

        self.func.switch_to_block(else_dst);
        self.func.ensure_inserted_block();
        let else_val = lower_branch(self, false);
        self.func.ins().jump(next_bb);
        let else_tail = self.func.current_block();

        self.func.switch_to_block(next_bb);
        self.func.ensure_inserted_block();
        self.func.seal_block(next_bb);

        ((then_tail, then_val), (else_tail, else_val))
    }
}

/* API wrappers of `FunctionBuilder` */
impl<'c> MainLowerContext<'_, 'c> {
    pub(crate) fn dfg(&self) -> &DataFlowGraph {
        &self.func.func.dfg
    }
    pub(crate) fn dfg_mut(&mut self) -> &mut DataFlowGraph {
        &mut self.func.func.dfg
    }

    pub(crate) fn get_srcloc(&self) -> SourceLoc {
        self.func.get_srcloc()
    }
    pub(crate) fn set_srcloc(&mut self, loc: SourceLoc) {
        self.func.set_srcloc(loc)
    }

    pub(crate) fn current_block(&self) -> Block {
        self.func.current_block()
    }
    pub(crate) fn create_block(&mut self) -> Block {
        self.func.create_block()
    }
    pub(crate) fn switch_to_block(&mut self, block: Block) {
        self.func.switch_to_block(block)
    }
    pub(crate) fn seal_block(&mut self, block: Block) {
        self.func.seal_block(block)
    }
    pub(crate) fn ensured_sealed(&mut self) {
        self.func.ensured_sealed()
    }

    pub(crate) fn ins(&mut self) -> InsertBuilder<'_, FuncInstBuilder<'_, 'c>> {
        self.func.ins()
    }
    pub(crate) fn fconst(&mut self, val: Ieee64) -> Value {
        self.func.fconst(val)
    }
    pub(crate) fn iconst(&mut self, val: i32) -> Value {
        self.func.iconst(val)
    }
    pub(crate) fn sconst(&mut self, val: &str) -> Value {
        self.func.sconst(val)
    }
}
