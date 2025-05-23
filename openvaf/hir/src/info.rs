//! Collect the metadata of a module: parameters and op variables.

use indexmap::IndexMap;
use syntax::{ast, name::SmolStr, AstNode};

use crate::diagnostics::{ConsoleSink, DiagnosticSink};
use crate::{BaseDB, CompilationDB, FileId};
use crate::{Module, ParamSysFun, Parameter, Variable};

mod module;
mod std_attrs;

#[cfg(test)]
mod tests;

pub use module::collect_modules;

pub struct ModuleInfo {
    pub module: Module,
    pub params: IndexMap<Parameter, ParamInfo, ahash::RandomState>,
    pub param_sysfuns: IndexMap<ParamSysFun, Vec<SmolStr>, ahash::RandomState>,
    pub op_vars: IndexMap<Variable, OpVarInfo, ahash::RandomState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParamInfo {
    pub name: SmolStr,
    pub aliases: Vec<SmolStr>,
    pub units: String,
    pub desc: String,
    pub group: String,
    pub is_instance: bool,
    pub multiplicity: Multiplicity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpVarInfo {
    pub units: String,
    pub desc: String,
    pub multiplicity: Multiplicity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Multiplicity {
    #[default]
    None,
    Multiply,
    Divide,
}
