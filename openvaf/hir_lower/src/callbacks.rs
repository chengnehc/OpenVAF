use hir::{Node, Parameter};
use mir::{FunctionSignature, Ieee64, Param, Spur};

use crate::fmt::{DisplayKind, FmtArg};
use crate::params::ParamInfoKind;
use crate::LimitState;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub enum CallBackKind {
    Print { kind: DisplayKind, fmt_args: Box<[FmtArg]> },
    SimParam,             // $simparam without fallback
    SimParamOpt,          // $simparam with optional fallback
    SimParamStr,          // $simparam$str()
    TimeDerivative,       // ddt()
    NodeDerivative(Node), // ddx(expr, V(node)), special case when the unknown is node potential probe
    Derivative(Param),    // other types of ddx()
    CollapseHint(Node, Option<Node>),
    LimDiscontinuity,                           // $discontinuity (unimplemented)
    Analysis,                                   // analysis()
    BuiltinLimit { name: Spur, num_args: u32 }, // $limit
    StoreLimit(LimitState),
    WhiteNoise { name: Spur, idx: u32 },
    FlickerNoise { name: Spur, idx: u32 },
    NoiseTable(Box<NoiseTable>),
    ParamInfo(ParamInfoKind, Parameter), // for parameter setup
}

impl CallBackKind {
    pub fn signature(&self) -> FunctionSignature {
        match self {
            CallBackKind::SimParam => FunctionSignature {
                name: "simparam".to_owned(),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::SimParamOpt => FunctionSignature {
                name: "simparam_opt".to_owned(),
                params: 2,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::SimParamStr => FunctionSignature {
                name: "simparam_str".to_owned(),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::TimeDerivative => FunctionSignature {
                name: "ddt".to_owned(),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::Derivative(param) => FunctionSignature {
                name: format!("ddx_{param}"),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::NodeDerivative(node) => FunctionSignature {
                name: format!("ddx_node_{node:?}"),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::ParamInfo(kind, param) => FunctionSignature {
                name: format!("set_{kind:?}({param:?})"),
                params: 0,
                returns: 0,
                has_side_effects: true,
            },
            CallBackKind::CollapseHint(hi, lo) => FunctionSignature {
                name: format!("collapse_{hi:?}_{lo:?}"),
                params: 0,
                returns: 0,
                has_side_effects: true,
            },
            CallBackKind::Print { kind, fmt_args } => FunctionSignature {
                name: format!("{kind:?}"),
                params: fmt_args.len() as u16 + 1,
                returns: 0,
                has_side_effects: true,
            },
            CallBackKind::BuiltinLimit { name, num_args } => FunctionSignature {
                name: format!("$limit[{name:?}]"),
                params: *num_args as u16,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::StoreLimit(state) => FunctionSignature {
                name: format!("$store[{state:?}]"),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::LimDiscontinuity => FunctionSignature {
                name: "$discontinuity[-1]".to_owned(),
                params: 0,
                returns: 0,
                has_side_effects: true,
            },
            CallBackKind::Analysis => FunctionSignature {
                name: "analysis".to_owned(),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::WhiteNoise { name, .. } => FunctionSignature {
                name: format!("white_noise({name:?})"),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::FlickerNoise { name, .. } => FunctionSignature {
                name: format!("flicker_noise({name:?})"),
                params: 2,
                returns: 1,
                has_side_effects: false,
            },
            CallBackKind::NoiseTable(table) => FunctionSignature {
                name: format!(
                    "table_noise{}({:?}, {:?})",
                    if table.log { "log" } else { "" },
                    table.name,
                    &table.vals
                ),
                params: 1,
                returns: 1,
                has_side_effects: false,
            },
        }
    }

    pub const fn is_noise(&self) -> bool {
        matches!(
            self,
            CallBackKind::WhiteNoise { .. }
                | CallBackKind::FlickerNoise { .. }
                | CallBackKind::NoiseTable(_)
        )
    }

    pub const fn is_op_dependent(&self) -> bool {
        matches!(
            self,
            CallBackKind::SimParam
                | CallBackKind::SimParamOpt
                | CallBackKind::SimParamStr
                | CallBackKind::Analysis
                | CallBackKind::LimDiscontinuity
                | CallBackKind::StoreLimit(_)
                | CallBackKind::BuiltinLimit { .. }
        )
    }

    pub const fn ignore_if_op_dependent(&self) -> bool {
        matches!(self, CallBackKind::CollapseHint(_, _))
    }

    pub const fn tracked(&self) -> bool {
        !matches!(self, CallBackKind::Print { .. })
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct NoiseTable {
    pub name: Spur,
    pub log: bool,
    pub vals: Box<[(Ieee64, Ieee64)]>,
    idx: u32,
}

impl NoiseTable {
    // TODO: read from disk
    pub fn new(
        vals: impl IntoIterator<Item = (f64, f64)>,
        log: bool,
        name: Spur,
        idx: u32,
    ) -> Self {
        let mut vals: Vec<(Ieee64, Ieee64)> = if log {
            vals.into_iter().map(|(f, pwr)| (f.into(), pwr.into())).collect()
        } else {
            vals.into_iter().map(|(f, pwr)| (f.log10().into(), pwr.into())).collect()
        };
        vals.sort_unstable_by(|(f1, _), (f2, _)| f1.partial_cmp(f2).unwrap());
        vals.dedup_by_key(|(f, _)| *f);
        Self { name, log, vals: vals.into_boxed_slice(), idx }
    }
}
