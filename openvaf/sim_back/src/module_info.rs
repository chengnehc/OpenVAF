//! Metadata of compiled module

use ahash::{AHashSet, RandomState};
use hir::diagnostics::{BaseDB, ConsoleSink, Diagnostic, FileId, Label, LabelStyle, Report};
use hir::{
    CompilationDB, CompilationUnit, DiagnosticSink, Module, ParamSysFun, Parameter,
    ResolvedAliasParam, ScopeDef, Variable,
};
use indexmap::IndexMap;
use smol_str::SmolStr;
use syntax::ast::{self, Expr};
use syntax::sourcemap::FileSpan;
use syntax::AstNode;

#[cfg(test)]
mod tests;

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

    let res = cu
        .modules(db)
        .into_iter()
        .map(|module| ModuleInfo::collect(db, cu, module, all_vars_op, sink))
        .collect();
    if sink.summary(&name) {
        return None;
    }

    Some(res)
}

pub struct ModuleInfo {
    pub module: Module,
    pub params: IndexMap<Parameter, ParamInfo, RandomState>,
    pub param_sysfuns: IndexMap<ParamSysFun, Vec<SmolStr>, RandomState>,
    pub op_vars: IndexMap<Variable, OpVar, RandomState>,
}

impl ModuleInfo {
    fn collect(
        db: &CompilationDB,
        cu: CompilationUnit,
        module: Module,
        all_vars_op: bool,
        sink: &mut ConsoleSink,
    ) -> ModuleInfo {
        let mut params: IndexMap<Parameter, ParamInfo, RandomState> = IndexMap::default();
        let mut param_sysfuns: IndexMap<ParamSysFun, Vec<SmolStr>, RandomState> =
            IndexMap::default();
        let mut op_vars = IndexMap::default();

        let mut decls = module.rec_declarations(db);
        let mut resolved_attrs = AHashSet::new();
        let mut add_diagnostic = |attr: ast::Attr, diag: &dyn Diagnostic| {
            if resolved_attrs.insert(attr.syntax().text_range()) {
                sink.add_diagnostic(diag, cu.root_file(), db)
            }
        };
        let ast = cu.ast_cache(db);

        while let Some((name, dec)) = decls.next() {
            match dec {
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
                    let units = units
                        .and_then(|attr| {
                            let lit = attr.val().and_then(|e| e.as_str_literal());
                            if lit.is_none() {
                                add_diagnostic(attr.clone(), &IllegalAttr { attr });
                            }
                            lit
                        })
                        .unwrap_or_default();
                    let desc = desc
                        .and_then(|attr| {
                            let lit = attr.val().and_then(|e| e.as_str_literal());
                            if lit.is_none() {
                                add_diagnostic(attr.clone(), &IllegalAttr { attr });
                            }
                            lit
                        })
                        .unwrap_or_default();

                    op_vars.insert(var, OpVar { units, desc });
                }

                ScopeDef::Parameter(param) => {
                    let units = param
                        .get_attr(db, &ast, "units")
                        .and_then(|attr| {
                            let lit = attr.val().and_then(|e| e.as_str_literal());
                            if lit.is_none() {
                                add_diagnostic(attr.clone(), &IllegalAttr { attr });
                            }
                            lit
                        })
                        .unwrap_or_default();

                    let desc = param
                        .get_attr(db, &ast, "desc")
                        .and_then(|attr| {
                            let lit = attr.val().and_then(|e| e.as_str_literal());
                            if lit.is_none() {
                                add_diagnostic(attr.clone(), &IllegalAttr { attr });
                            }
                            lit
                        })
                        .unwrap_or_default();

                    // "group" is not a standard attribute, but is used by VerilogAE
                    // for parameter extraction
                    let group = param
                        .get_attr(db, &ast, "group")
                        .and_then(|attr| {
                            let lit = attr.val().and_then(|e| e.as_str_literal());
                            if lit.is_none() {
                                add_diagnostic(attr.clone(), &IllegalAttr { attr });
                            }
                            lit
                        })
                        .unwrap_or_default();

                    // "type" is not a standard attribute, but is used by compact models
                    // comprehensively to distinguish between instance and model params.
                    let type_ = param.get_attr(db, &ast, "type").and_then(|attr| {
                        let lit = attr.val().and_then(|e| e.as_str_literal());
                        if lit.is_none() {
                            add_diagnostic(attr.clone(), &IllegalAttr { attr });
                        }
                        lit
                    });
                    let is_instance = match type_.as_deref() {
                        Some("instance") => true,
                        Some("model") | None => false,
                        Some(found) => {
                            let attr = param.get_attr(db, &ast, "type").unwrap();
                            add_diagnostic(
                                attr.clone(),
                                &UnknownType { expr: attr.val().unwrap(), found },
                            );
                            false
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

        ModuleInfo { module, params, param_sysfuns, op_vars }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParamInfo {
    pub name: SmolStr,
    pub aliases: Vec<SmolStr>,
    pub units: String,
    pub desc: String,
    pub group: String,
    pub is_instance: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpVar {
    pub units: String,
    pub desc: String,
    // TODO(JW) add standard attribute 'multiplicity'. [LRM 2.9.2]
    // pub multiplicity
}

struct IllegalAttr {
    attr: ast::Attr,
}

impl Diagnostic for IllegalAttr {
    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let FileSpan { range, file } = db
            .parse(root_file)
            .to_file_span(self.attr.syntax().text_range(), &db.sourcemap(root_file));
        Report::error()
            .with_message(format!(
                "illegal expression supplied to '{}' attribute; expected a string literal",
                self.attr.name().unwrap(),
            ))
            .with_labels(vec![Label {
                style: LabelStyle::Primary,
                file_id: file,
                range: range.into(),
                message: "expected a string literal".to_owned(),
            }])
    }
}

struct UnknownType<'a> {
    expr: Expr,
    found: &'a str,
}

impl Diagnostic for UnknownType<'_> {
    fn build_report(&self, root_file: FileId, db: &dyn BaseDB) -> Report {
        let FileSpan { range, file } = db.parse(root_file).to_file_span(
            self.expr.syntax().parent().unwrap().text_range(),
            &db.sourcemap(root_file),
        );
        Report::warning()
            .with_message(format!(
                "unknown type \"{}\" expected \"model\" or \"instance\"",
                self.found
            ))
            .with_labels(vec![Label {
                style: LabelStyle::Primary,
                file_id: file,
                range: range.into(),
                message: "unknown type".to_owned(),
            }])
    }
}
