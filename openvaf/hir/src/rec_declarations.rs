//! Define an iterator that recursively visit all of the declarations within a Scope,
//! that is, module, named block, analog function.

use std::sync::Arc;
use std::{iter, mem};

use hir_def::nameres::{DefMap, LocalScopeId, ScopeItem};
use syntax::name::{Name, SmolStr};

use crate::{AliasParam, Block, Branch, Module, Node, Parameter, ScopeDef, Variable};
use crate::{CompilationDB, HirDefDB};

struct Scope {
    iter: indexmap::map::Iter<'static, Name, ScopeItem>,
    def: Option<(Name, ScopeDef)>,
}

impl Scope {
    fn new(def_map: &Arc<DefMap>, scope: LocalScopeId, def: Option<(Name, ScopeDef)>) -> Scope {
        let iter = def_map[scope].decls().iter();
        // # safety: def_map is a immutable/an arc that will live at least as long as the scope
        #[allow(clippy::missing_transmute_annotations)]
        let iter = unsafe { mem::transmute(iter) };

        Scope { iter, def }
    }
}

/// An iterator that recursively visit all of the declarations within a scope.
pub struct RecDeclarations<'a> {
    db: &'a CompilationDB,
    /// identifier path represented by a list of name
    path: Vec<Name>,
    /// work stack for child scopes
    stack: Vec<Scope>,
}

impl<'a> RecDeclarations<'a> {
    pub(super) fn new(scope: super::Scope, db: &'a CompilationDB) -> RecDeclarations<'a> {
        let (scope_id, def_map) = scope.def_map_and_scope(db);
        let scope = Scope::new(&def_map, scope_id, None);

        RecDeclarations { db, path: Vec::new(), stack: vec![scope] }
    }

    pub fn to_path(&self, name: Name) -> SmolStr {
        if self.path.is_empty() {
            // fast path
            return name.into();
        }
        self.path.iter().flat_map(|path| [path, "."]).chain(iter::once(&*name)).collect()
    }
}

impl Iterator for RecDeclarations<'_> {
    type Item = (Name, ScopeDef);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let scope = self.stack.last_mut()?;
            if let Some((name, &item)) = scope.iter.next() {
                let def = match item {
                    ScopeItem::ModuleId(id) => ScopeDef::Module(Module { id }),
                    ScopeItem::NodeId(id) => ScopeDef::Node(Node { id }),
                    ScopeItem::BranchId(id) => ScopeDef::Branch(Branch { id }),
                    ScopeItem::VarId(id) => ScopeDef::Variable(Variable { id }),
                    ScopeItem::ParamId(id) => ScopeDef::Parameter(Parameter { id }),
                    ScopeItem::AliasParamId(id) => ScopeDef::AliasParam(AliasParam { id }),
                    ScopeItem::BlockId(id) => {
                        if let Some(def_map) = self.db.block_def_map(id) {
                            let entry = def_map.entry_scope();
                            let block = (name.clone(), ScopeDef::Block(Block { id }));
                            self.stack.push(Scope::new(&def_map, entry, Some(block)))
                        }
                        continue;
                    }
                    _ => continue,
                };
                return Some((name.clone(), def));
            } else {
                self.path.pop(); // jump back to parent scope
                let scope = self.stack.pop()?;
                if let Some(def) = scope.def {
                    return Some(def);
                }
            }
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use expect_test::{expect, Expect};

//     fn assert(src: &str, expect: Expect) {
//         let db = CompilationDB::new_from_vfs(&src).unwrap();
//         let cu = db.compilation_unit();
//         let modules = cu.modules(&db);
//         let decls: Vec<_> = modules[0].rec_declarations(&db).collect();

//         expect.assert_debug_eq(&decls);
//     }

//     #[test]
//     fn smoke_test() {
//         let src = r#"
//         module test(inout d, inout s);
//             electrical d, s;
//             branch (d, s) br_d_s;
//             parameter real outer_param = 100 from (0:inf);
//             real outer_var = 0;

//             analog begin
//                 begin: myscope
//                     parameter real inner_param = Rd;
//                     real inner_var = 1.5 * inner;
//                 end
//                 outer_var = myscope.inner_var;
//             end
//         endmodule
//         "#;

//         assert(&src, expect);
//     }
// }
