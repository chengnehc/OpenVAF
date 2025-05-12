//! Lowers the HIR to build MIR. This serves as the only bridge between
//! various MIR crates and the HIR.

use stdx::packed_option::PackedOption;
use stdx::{impl_debug_display, impl_idx_from};

use ahash::AHashSet;
use hir::{
    Branch, BranchWrite, CompilationDB, Module, Node, ParamSysFun, Parameter, Type, Variable,
};
use indexmap::IndexMap;
use lasso::Rodeo;
use mir::builder::InstBuilder;
use mir::{DataFlowGraph, FuncRef, Function, Inst, KnownDerivatives, Param, Value};
use mir_build::{FunctionBuilder, FunctionBuilderContext, RetBuilder};
use typed_index_collections::TiVec;
use typed_indexmap::{TiMap, TiSet};

mod body;
mod callbacks;
mod ctx;
mod expr;
mod parameters;
mod state;
mod stmt;

pub mod fmt;

use body::BodyLowerContext;
use ctx::MainLowerContext;

pub use callbacks::{CallBackKind, NoiseTable};
pub use parameters::ParamInfoKind;

/// A builder to lower HIR into MIR
pub struct MirBuilder<'a> {
    module: Module,
    db: &'a CompilationDB,
    func_ctxt: Option<&'a mut FunctionBuilderContext>,
    // predicate indicating whether a `Place` should be treated as output
    is_output: &'a dyn Fn(PlaceKind) -> bool,
    // store required output variables
    required_vars: &'a mut dyn Iterator<Item = Variable>,
    // for parameter extraction backend
    tagged_reads: AHashSet<Variable>,
    // for simulator backend
    tag_writes: bool,
    // for simulator backend
    lower_equations: bool,
}

impl<'a> MirBuilder<'a> {
    /// Create a new MIR builder. By default, it:
    /// - uses default function builder context
    /// - does not lower equations
    /// - does not tag variable writes
    /// - does not tag variable reads
    ///
    /// Use builder methods to configure the MIR builder for extended functions.
    pub fn new(
        db: &'a CompilationDB,
        module: Module,
        is_output: &'a dyn Fn(PlaceKind) -> bool,
        required_vars: &'a mut dyn Iterator<Item = Variable>,
    ) -> MirBuilder<'a> {
        MirBuilder {
            module,
            db,
            is_output,
            required_vars,
            func_ctxt: None,
            tagged_reads: AHashSet::new(),
            tag_writes: false,
            lower_equations: false,
        }
    }

    pub fn with_func_builder_context(mut self, func_ctxt: &'a mut FunctionBuilderContext) -> Self {
        self.func_ctxt = Some(func_ctxt);
        self
    }

    // pub fn add_tag_read(&mut self, var: Variable) -> bool {
    //     self.tagged_reads.insert(var)
    // }

    pub fn with_tagged_reads(mut self, vars: AHashSet<Variable>) -> Self {
        self.tagged_reads = vars;
        self
    }

    pub fn with_write_tags(mut self) -> Self {
        self.tag_writes = true;
        self
    }

    pub fn with_equations(mut self) -> Self {
        self.lower_equations = true;
        self
    }

    pub fn build(self, literals: &mut Rodeo) -> (Function, HirInterner) {
        let mut function = Function::default();
        let mut interner = HirInterner::default();
        let func_ctxt = match self.func_ctxt {
            Some(func_ctxt) => func_ctxt,
            None => &mut FunctionBuilderContext::new(),
        };
        let func_builder =
            FunctionBuilder::new(&mut function, literals, func_ctxt, self.tag_writes);
        let mut ctxt = MainLowerContext::new(self.db, func_builder, &mut interner)
            .with_equations(self.lower_equations)
            .with_tagged_reads(self.tagged_reads);

        let path = self.module.name(self.db);
        let body = self.module.analog_initial_body(self.db);
        let mut body_ctxt = BodyLowerContext { ctxt: &mut ctxt, body: body.borrow(), path: &path };
        body_ctxt.lower_entry_stmts();

        let body = self.module.analog_body(self.db);
        body_ctxt.with_body(body.borrow()).lower_entry_stmts();

        // declare places at entry block for op variables
        for var in self.required_vars {
            ctxt.dec_place(PlaceKind::Var(var));
        }

        // ensure optbarriers for outputs
        // after lowering, the function builder should be positioned at
        // the block before exit block, optbarriers are created here.
        //
        // dbg!(&ctxt.func.cursor().position());
        ctxt.intern.outputs = ctxt
            .places
            .iter_enumerated()
            .map(|(place, kind)| {
                if (self.is_output)(*kind) {
                    let val = ctxt.func.use_var(place);
                    let val = ctxt.func.ins().ensure_optbarrier(val);
                    (*kind, val.into())
                } else {
                    (*kind, None.into())
                }
            })
            .collect();

        ctxt.func.ins().ret();
        ctxt.func.finalize();

        (function, interner)
    }
}

/// A mapping between abstractions used in the MIR and the corresponding information
/// from the HIR，which allows the MIR to remain independent of the frontend/HIR.
#[derive(Debug, PartialEq, Default, Clone)]
pub struct HirInterner {
    /// Mapping from MIR function `Param`s to SSA `Value`s
    pub params: TiMap<Param, ParamKind, Value>,
    /// Mapping from output `Place`s to possible SSA `Value`s
    pub outputs: IndexMap<PlaceKind, PackedOption<Value>, ahash::RandomState>,
    /// Callback function declarations in the MIR function
    pub callbacks: TiSet<FuncRef, CallBackKind>,
    /// Caller instructions of each callback function
    pub callback_callers: TiVec<FuncRef, Vec<Inst>>,
    /// Implicit equation unknowns needed by simulator backend
    pub implicit_equations: TiVec<ImplicitEquation, ImplicitEquationKind>,
    /// Internal states induced by $limit
    /// Mapping from unknown value to (lim_val, negate_lim)
    pub lim_state: TiMap<LimitState, Value, Vec<(Value, bool)>>,
    // JW: for VerilogAE backend
    pub tagged_reads: IndexMap<Value, Variable, ahash::RandomState>,
}

const NEG: bool = true;
const POS: bool = false;

impl HirInterner {
    /// Get all known derivatives (ddx calls) explicitly specified by the user.
    ///
    /// This method can be used by both parameter extraction backend and simulator backend.
    /// A major difference between sim_back and verilogae is that sim_back requires full Jacobi.
    pub fn derivative_info(&self, func: impl AsRef<Function>, sim_back: bool) -> KnownDerivatives {
        let func = func.as_ref();
        let KnownDerivatives { mut unknowns, mut ddx_calls } = KnownDerivatives::default();

        let mut has_ddx_call = |ddx, unk, neg| match self.callbacks.index_of(&ddx) {
            Some(ddx_call) => {
                // insert ddx calls
                let (pos_dst, neg_dst) = ddx_calls.entry(ddx_call).or_default();
                let dst = if neg { neg_dst } else { pos_dst };
                dst.insert(unk, func.dfg.num_values());
                true
            }
            None => false,
        };

        for (param, &kind, val) in self.live_params(&func.dfg) {
            let mut node_required = |node, neg| {
                let ddx = CallBackKind::NodeDerivative(node);
                has_ddx_call(ddx, unknowns.len().into(), neg)
            };
            let unknown_required = match kind {
                // For simulator backend, parameter typed branch potential probe, flow
                // probe and implicit unknowns are always required.
                ParamKind::Potential { hi, lo: Some(lo) } => {
                    sim_back | node_required(hi, POS) | node_required(lo, NEG)
                }
                ParamKind::Potential { hi, lo: None } => sim_back | node_required(hi, POS),
                ParamKind::Flow(_) | ParamKind::ImplicitUnknown(_) => sim_back,
                _ => {
                    let ddx = CallBackKind::Derivative(param);
                    has_ddx_call(ddx, unknowns.len().into(), POS)
                }
            };

            if unknown_required {
                unknowns.insert(val);
            }
        }

        // Deal with limit state
        for (param, vals) in self.lim_state.iter() {
            for &(val, neg) in vals {
                let param = func.dfg.value_def(*param).unwrap_param();
                let ddx = CallBackKind::Derivative(param);
                let mut required = has_ddx_call(ddx, unknowns.len().into(), neg);
                let mut node_required = |node, neg| {
                    let ddx = CallBackKind::NodeDerivative(node);
                    has_ddx_call(ddx, unknowns.len().into(), neg)
                };

                match *self.params.get_index(param).unwrap().0 {
                    ParamKind::Potential { hi, lo: None } => required |= node_required(hi, neg),
                    ParamKind::Potential { hi, lo: Some(lo) } => {
                        required |= node_required(hi, false) | node_required(lo, !neg);
                    }
                    _ => (),
                };

                if required | sim_back {
                    unknowns.insert(val);
                }
            }
        }

        KnownDerivatives { unknowns, ddx_calls }
    }

    pub fn ensure_param(&mut self, func: &mut impl AsMut<Function>, kind: ParamKind) -> Value {
        Self::ensure_param_(&mut self.params, func, kind)
    }

    // FIXME(JW) this is to work around borrow checker, maybe there's more idiomatic way doing this.
    pub fn ensure_param_(
        params: &mut TiMap<Param, ParamKind, Value>,
        func: &mut impl AsMut<Function>,
        kind: ParamKind,
    ) -> Value {
        let len = params.len();
        let entry = params.raw.entry(kind);
        *entry.or_insert_with(|| func.as_mut().dfg.make_param(len.into()))
    }

    pub fn is_param_live(&self, func: impl AsRef<Function>, kind: &ParamKind) -> bool {
        self.params.get(kind).is_some_and(|val| !func.as_ref().dfg.value_dead(*val))
    }

    /// Return an iterator over the function parameters that are alive, i.e., used by some instruction.
    pub fn live_params<'a>(
        &'a self,
        dfg: &'a DataFlowGraph,
    ) -> impl Iterator<Item = (Param, &'a ParamKind, Value)> + Clone {
        self.params.iter_enumerated().filter_map(|(param, (kind, val))| {
            (!dfg.value_dead(*val)).then_some((param, kind, *val))
        })
    }
}

/// Different kinds of MIR function parameters
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamKind {
    Potential { hi: Node, lo: Option<Node> },
    Flow(FlowKind),
    ImplicitUnknown(ImplicitEquation),
    Param(Parameter),
    ParamSysFun(ParamSysFun),
    Temperature,
    PortConnected { port: Node },
    ParamGiven { param: Parameter },
    Abstime,
    EnableIntegration,
    EnableLim,
    HiddenState(Variable), // TODO(JW): we now create a hidden state for every variable
    PrevState(LimitState),
    NewState(LimitState),
}

impl ParamKind {
    pub fn is_op_dependent(&self) -> bool {
        matches!(
            self,
            ParamKind::Potential { .. }
                | ParamKind::Flow(_)
                | ParamKind::ImplicitUnknown(_)
                | ParamKind::Abstime
                | ParamKind::EnableIntegration
                | ParamKind::EnableLim
                | ParamKind::HiddenState(_)
                | ParamKind::PrevState(_)
                | ParamKind::NewState(_)
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FlowKind {
    Branch(Branch),
    Unnamed { hi: Node, lo: Option<Node> },
    Port(Node),
}
impl From<BranchWrite> for FlowKind {
    fn from(kind: BranchWrite) -> Self {
        match kind {
            BranchWrite::Named(branch) => FlowKind::Branch(branch),
            BranchWrite::Unnamed { hi, lo } => FlowKind::Unnamed { hi, lo },
        }
    }
}
impl TryFrom<FlowKind> for BranchWrite {
    type Error = ();
    // FIXME(JW) should not use () as Error type
    fn try_from(kind: FlowKind) -> Result<BranchWrite, ()> {
        match kind {
            FlowKind::Branch(branch) => Ok(BranchWrite::Named(branch)),
            FlowKind::Unnamed { hi, lo } => Ok(BranchWrite::Unnamed { hi, lo }),
            FlowKind::Port(_) => Err(()),
        }
    }
}

/// Different kinds of places(variables) used in the MIR function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlaceKind {
    Var(Variable),
    FunctionReturn(hir::Function),
    FunctionArg(hir::FunctionArg),
    /// A contribution statement LHS: source branch
    Contribute {
        branch: BranchWrite,
        is_potential: bool,
    },
    /// A flag indicating whether source branch is a potential source
    IsPotential(BranchWrite),
    /// The residual corresponding to an implicit equation
    ImplicitResidual {
        equation: ImplicitEquation,
        reactive: bool,
    },
    /// A flag indicating whether an implicit equation should be collapsed
    CollapseImplicitEquation(ImplicitEquation),
    BoundStep,
    /// A parameter during initialization is mutable: write default in case not given
    Param(Parameter),
    ParamMin(Parameter),
    ParamMax(Parameter),
}

impl From<hir::AssignmentLhs> for PlaceKind {
    fn from(hir: hir::AssignmentLhs) -> Self {
        match hir {
            hir::AssignmentLhs::Variable(var) => PlaceKind::Var(var),
            hir::AssignmentLhs::FunctionReturn(fun) => PlaceKind::FunctionReturn(fun),
            hir::AssignmentLhs::FunctionArg(arg) => PlaceKind::FunctionArg(arg),
        }
    }
}

impl PlaceKind {
    pub fn ty(&self, db: &CompilationDB) -> Type {
        use PlaceKind::*;

        match *self {
            ParamMin(param) | ParamMax(param) | Param(param) => param.ty(db),
            Var(var) => var.ty(db),
            FunctionReturn(fun) => fun.return_ty(db),
            FunctionArg(arg) => arg.ty(db),
            IsPotential(_) | CollapseImplicitEquation(_) => Type::Bool,
            Contribute { .. } | ImplicitResidual { .. } | BoundStep => Type::Real,
        }
    }

    // /// This place is only used in intialization function
    // pub fn is_init_only(&self) -> bool {
    //     matches!(self, Self::CollapseImplicitEquation(_))
    // }
}

/// Implicit equations, which are generated by analog operators
/// such as ddt, idt and noise sources.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImplicitEquation(u32);
impl_idx_from!(ImplicitEquation(u32));
impl_debug_display! {
    match ImplicitEquation {ImplicitEquation(id) => "inode{id}";}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImplicitEquationKind {
    Ddt,
    Idt(IdtKind),
    NoiseSrc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IdtKind {
    Basic,
    Ic,
    Assert,
    Modulus,
    ModulusOffset,
}
impl IdtKind {
    pub const fn num_params(self) -> u16 {
        match self {
            IdtKind::Basic => 1,
            IdtKind::Ic => 2,
            IdtKind::Assert | IdtKind::Modulus => 3,
            IdtKind::ModulusOffset => 4,
        }
    }
    pub const fn has_ic(self) -> bool {
        !matches!(self, IdtKind::Basic)
    }
    pub const fn has_assert(self) -> bool {
        matches!(self, IdtKind::Assert)
    }
    pub const fn has_modulus(self) -> bool {
        matches!(self, IdtKind::Modulus | IdtKind::ModulusOffset)
    }
    pub const fn has_offset(self) -> bool {
        matches!(self, IdtKind::ModulusOffset)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LimitState(u32);
impl_idx_from!(LimitState(u32));
impl_debug_display! {
    match LimitState {LimitState(id) => "lim_state{id}";}
}
