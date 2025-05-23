use super::std_attrs::StdAttrDiagnostic::*;
use super::*;

use ahash::AHashSet;

use crate::{CompilationUnit, ResolvedAliasParam, ScopeDef};

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

                    op_vars.insert(var, OpVarInfo { units, desc, multiplicity });
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
