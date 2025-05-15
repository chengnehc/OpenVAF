//! Define an iterator that recursively visit all of the declarations within a Scope,
//! that is, module, named block, analog function.

use std::iter;
use std::mem::transmute;
use std::ops::Deref;
use std::sync::Arc;

use hir_def::nameres::{self, DefMap, LocalScopeId, ScopeItem};
use smol_str::SmolStr;
use syntax::name::Name;

use crate::{
    AliasParam, Block, Branch, CompilationDB, HirDefDB, Module, Node, Parameter, ScopeDef, Variable,
};

struct Scope {
    //_def_map: Arc<DefMap>,
    iter: indexmap::map::Iter<'static, Name, nameres::ScopeItem>,
    def: Option<(Name, ScopeDef)>,
}
impl Scope {
    fn new(def_map: Arc<DefMap>, scope: LocalScopeId, def: Option<(Name, ScopeDef)>) -> Scope {
        // safety: def_map is a immutable/an arc that will live at least as long as the scope
        let iter = def_map[scope].declarations.iter();
        let iter = unsafe { transmute(iter) };

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

        RecDeclarations { db, path: Vec::new(), stack: vec![Scope::new(def_map, scope_id, None)] }
    }

    pub fn to_path(&self, name: Name) -> SmolStr {
        if self.path.is_empty() {
            // fast path
            return name.into();
        }
        self.path.iter().flat_map(|path| [path, "."]).chain(iter::once(name.deref())).collect()
    }

    pub fn current_path(&self) -> &[Name] {
        &self.path
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
                            self.stack.push(Scope::new(def_map, entry, Some(block)))
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
                continue;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::{expect, Expect};
    use std::fs;
    use stdx::integration_test_dir;

    fn assert(src: &str, expect: Expect) {
        let db = CompilationDB::new_from_vfs(&src).unwrap();
        let cu = db.compilation_unit();
        let modules = cu.modules(&db);
        let decls: Vec<_> = modules[0].rec_declarations(&db).collect();

        expect.assert_debug_eq(&decls);
    }

    #[test]
    fn smoke_test() {
        let src = r#"
        module test(inout d, inout s);
            electrical d, s;
            branch (d, s) br_d_s;
            parameter real outer_param = 100 from (0:inf);
            real outer_var = 0;

            analog begin
                begin: myscope
                    parameter real inner_param = Rd;
                    real inner_var = 1.5 * inner;
                end
                outer_var = myscope.inner_var;
            end
        endmodule
        "#;

        let expect = expect![[r#"
        [
            (
                Name(
                    "d",
                ),
                Node(
                    node0,
                ),
            ),
            (
                Name(
                    "s",
                ),
                Node(
                    node1,
                ),
            ),
            (
                Name(
                    "br_d_s",
                ),
                Branch(
                    BranchId(0),
                ),
            ),
            (
                Name(
                    "outer_param",
                ),
                Parameter(
                    Parameter {
                        id: ParamId(
                            0,
                        ),
                    },
                ),
            ),
            (
                Name(
                    "outer_var",
                ),
                Variable(
                    VarId(0),
                ),
            ),
            (
                Name(
                    "inner_param",
                ),
                Parameter(
                    Parameter {
                        id: ParamId(
                            1,
                        ),
                    },
                ),
            ),
            (
                Name(
                    "inner_var",
                ),
                Variable(
                    VarId(1),
                ),
            ),
            (
                Name(
                    "myscope",
                ),
                Block(
                    BlockId(0),
                ),
            ),
        ]
        "#]];

        assert(&src, expect);
    }

    #[test]
    fn resistor() {
        let src = fs::read_to_string(integration_test_dir("RESISTOR").join("resistor.va")).unwrap();
        let expect = expect![[r#"
            [
                (
                    Name(
                        "A",
                    ),
                    Node(
                        node0,
                    ),
                ),
                (
                    Name(
                        "B",
                    ),
                    Node(
                        node1,
                    ),
                ),
                (
                    Name(
                        "br_a_b",
                    ),
                    Branch(
                        BranchId(0),
                    ),
                ),
                (
                    Name(
                        "R",
                    ),
                    Parameter(
                        Parameter {
                            id: ParamId(
                                0,
                            ),
                        },
                    ),
                ),
                (
                    Name(
                        "zeta",
                    ),
                    Parameter(
                        Parameter {
                            id: ParamId(
                                1,
                            ),
                        },
                    ),
                ),
                (
                    Name(
                        "tnom",
                    ),
                    Parameter(
                        Parameter {
                            id: ParamId(
                                2,
                            ),
                        },
                    ),
                ),
                (
                    Name(
                        "res",
                    ),
                    Variable(
                        VarId(0),
                    ),
                ),
                (
                    Name(
                        "vres",
                    ),
                    Variable(
                        VarId(1),
                    ),
                ),
            ]
        "#]];
        assert(&src, expect);
    }
}
