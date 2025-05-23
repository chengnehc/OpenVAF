//! Collect the metadata of a module: parameters and op variables.

use stdx::impl_display;

use ahash::{AHashSet, RandomState};
use indexmap::IndexMap;
use syntax::{ast, name::SmolStr, sourcemap::FileSpan, AstNode};

use crate::diagnostics::{ConsoleSink, Diagnostic, DiagnosticSink, Label, LabelStyle, Report};
use crate::{BaseDB, CompilationDB, CompilationUnit, FileId};
use crate::{Module, ParamSysFun, Parameter, ResolvedAliasParam, ScopeDef, Variable};

#[cfg(test)]
mod tests;

/// Emit frontend diagnostics and collect the information of the declared modules.
pub fn collect_modules(
    db: &CompilationDB,
    all_vars_op: bool,
    sink: &mut ConsoleSink,
) -> Option<Vec<ModuleInfo>> {
    let cu = db.compilation_unit();
    let name = cu.name(db);

    cu.collect_diagnostics(db, sink);
    if sink.summary(&name) {
        return None;
    }

    let module_infos = cu
        .modules(db)
        .into_iter()
        .map(|module| ModuleInfo::collect(db, cu, module, all_vars_op, sink))
        .collect();
    // report errors occurred during collecting module info
    if sink.summary(&name) {
        return None;
    }

    Some(module_infos)
}

pub struct ModuleInfo {
    pub module: Module,
    pub params: IndexMap<Parameter, ParamInfo, RandomState>,
    pub param_sysfuns: IndexMap<ParamSysFun, Vec<SmolStr>, RandomState>,
    pub op_vars: IndexMap<Variable, OpVar, RandomState>,
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
pub struct OpVar {
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

impl ModuleInfo {
    fn collect(
        db: &CompilationDB,
        cu: CompilationUnit,
        module: Module,
        all_vars_op: bool,
        sink: &mut ConsoleSink,
    ) -> ModuleInfo {
        let root_file = cu.root_file();
        let ast = cu.ast_cache(db);

        let mut params = IndexMap::default();
        let mut param_sysfuns: IndexMap<_, Vec<_>, _> = IndexMap::default();
        let mut op_vars = IndexMap::default();

        let mut resolved_attrs = AHashSet::new();
        let mut diagnostics = Vec::new();
        let mut diagnose = |attr: ast::Attr| {
            let lit = attr.val().and_then(|e| e.as_str_literal());
            if lit.is_none() && resolved_attrs.insert(attr.syntax().text_range()) {
                diagnostics.push(IllegalAttr { attr });
            }
            lit
        };

        let mut decls = module.rec_declarations(db);
        while let Some((name, decl)) = decls.next() {
            match decl {
                ScopeDef::Variable(var) => {
                    // 3.2.1 Output variables
                    //
                    // operating point variables must fulfill two properties
                    // * have a description or units attribute
                    // * belong to a module (not a block/function) -> no path

                    // check for units or description
                    let units = var.get_attr(db, &ast, "units");
                    let desc = var.get_attr(db, &ast, "desc");
                    if units.is_none() && desc.is_none() && !all_vars_op {
                        continue;
                    }
                    // check that we are not in a block
                    let name_len = name.len();
                    let path = decls.to_path(name);
                    if path.len() != name_len {
                        continue;
                    }
                    let units = units.and_then(|attr| diagnose(attr.clone())).unwrap_or_default();
                    let desc = desc.and_then(|attr| diagnose(attr.clone())).unwrap_or_default();

                    let multiplicity = var
                        .get_attr(db, &ast, "multiplicity")
                        .and_then(|attr| diagnose(attr.clone()));
                    let multiplicity = match multiplicity.as_deref() {
                        Some("multiply") => Multiplicity::Multiply,
                        Some("divide") => Multiplicity::Divide,
                        Some("none") | None => Multiplicity::None,
                        Some(found) => {
                            let attr = var.get_attr(db, &ast, "multiplicity").unwrap();
                            sink.add_diagnostic(
                                &UnknownMultiplicity { attr, found: found.to_owned() },
                                root_file,
                                db,
                            );
                            Multiplicity::None
                        }
                    };

                    op_vars.insert(var, OpVar { units, desc, multiplicity });
                }

                ScopeDef::Parameter(param) => {
                    let units = param
                        .get_attr(db, &ast, "units")
                        .and_then(|attr| diagnose(attr.clone()))
                        .unwrap_or_default();

                    let desc = param
                        .get_attr(db, &ast, "desc")
                        .and_then(|attr| diagnose(attr.clone()))
                        .unwrap_or_default();

                    // "group" is not a standard attribute, but is used by VerilogAE
                    // for parameter extraction
                    let group = param
                        .get_attr(db, &ast, "group")
                        .and_then(|attr| diagnose(attr.clone()))
                        .unwrap_or_default();

                    // "type" is not a standard attribute, but is used by compact models
                    // comprehensively to distinguish between instance and model params.
                    let type_ =
                        param.get_attr(db, &ast, "type").and_then(|attr| diagnose(attr.clone()));
                    let is_instance = match type_.as_deref() {
                        Some("instance") => true,
                        Some("model") | None => false,
                        Some(found) => {
                            let attr = param.get_attr(db, &ast, "type").unwrap();
                            sink.add_diagnostic(
                                &UnknownParamType { attr, found: found.to_owned() },
                                root_file,
                                db,
                            );
                            false
                        }
                    };

                    let multiplicity = param
                        .get_attr(db, &ast, "multiplicity")
                        .and_then(|attr| diagnose(attr.clone()));
                    let multiplicity = match multiplicity.as_deref() {
                        Some("multiply") => Multiplicity::Multiply,
                        Some("divide") => Multiplicity::Divide,
                        Some("none") | None => Multiplicity::None,
                        Some(found) => {
                            let attr = param.get_attr(db, &ast, "multiplicity").unwrap();
                            sink.add_diagnostic(
                                &UnknownMultiplicity { attr, found: found.to_owned() },
                                root_file,
                                db,
                            );
                            Multiplicity::None
                        }
                    };

                    params.insert(
                        param,
                        ParamInfo {
                            name: decls.to_path(name),
                            aliases: Vec::new(),
                            units,
                            desc,
                            group,
                            is_instance,
                            multiplicity,
                        },
                    );
                }

                ScopeDef::AliasParam(alias) => match alias.resolve(db).unwrap() {
                    ResolvedAliasParam::Parameter(param) => {
                        params.entry(param).or_default().aliases.push(decls.to_path(name))
                    }
                    ResolvedAliasParam::Sysfun(sysfun) => {
                        param_sysfuns.entry(sysfun).or_default().push(decls.to_path(name))
                    }
                },

                _ => (),
            }
        }

        sink.add_diagnostics(diagnostics.iter(), root_file, db);

        ModuleInfo { module, params, param_sysfuns, op_vars }
    }
}

/// Refer to [LRM 2.9.2] standard attributes
enum StdAttrDiagnostic {
    IllegalAttr { attr: ast::Attr },
    UnknownParamType { attr: ast::Attr, found: String },
    UnknownMultiplicity { attr: ast::Attr, found: String },
}

use StdAttrDiagnostic::*;
impl_display! {
    match StdAttrDiagnostic {
        IllegalAttr { attr } => "illegal expression supplied to '{}' attribute; expected a string literal", attr.name().unwrap();
        UnknownParamType { found, .. } => r#"unknown parameter type "{}"; expected "model" or "instance""#, found;
        UnknownMultiplicity { found, .. } => r#"unknown multiplicity attribute value "{}"; expected "multiply", "divide" or "none""#, found;

    }
}

impl Diagnostic for StdAttrDiagnostic {
    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let src_map = db.sourcemap(root_file);
        let parse = db.parse(root_file);

        let report = match self {
            IllegalAttr { attr } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::error().with_labels(vec![Label {
                    style: LabelStyle::Primary,
                    file_id: file,
                    range: range.into(),
                    message: "expected a string literal".to_owned(),
                }])
            }
            UnknownParamType { attr, .. } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::warning()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: file,
                        range: range.into(),
                        message: "unknown parameter type".to_owned(),
                    }])
                    .with_notes(
                        vec!["note: parameter type is set to 'model' by default".to_owned()],
                    )
            }
            UnknownMultiplicity { attr, .. } => {
                let FileSpan { range, file } =
                    parse.to_file_span(attr.syntax().text_range(), &src_map);

                Report::warning()
                    .with_labels(vec![Label {
                        style: LabelStyle::Primary,
                        file_id: file,
                        range: range.into(),
                        message: "unknown multiplicity attribute value".to_owned(),
                    }])
                    .with_notes(vec![
                        "note: multiplicity is set to 'none' by default, no scaling is performed"
                            .to_owned(),
                    ])
            }
        };

        report.with_message(self.to_string())
    }
}
