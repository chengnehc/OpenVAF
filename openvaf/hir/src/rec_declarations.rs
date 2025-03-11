//! Recursive declarations (?)

use std::iter;
use std::mem::transmute;
use std::ops::Deref;
use std::sync::Arc;

use hir_def::nameres::{self, DefMap, LocalScopeId, ScopeItemDef};
use smol_str::SmolStr;
use syntax::name::Name;

use crate::{
    AliasParam, Block, Branch, CompilationDB, HirDefDB, Module, Node, Parameter, ScopeDef, Variable,
};

struct Scope {
    _def_map: Arc<DefMap>,
    iter: indexmap::map::Iter<'static, Name, nameres::ScopeItemDef>,
    def: Option<(Name, ScopeDef)>,
}
impl Scope {
    fn new(def_map: Arc<DefMap>, scope: LocalScopeId, block: Option<(Name, ScopeDef)>) -> Scope {
        // safety: def_map is a immutable/an arc that will live at least as long as the scope
        let iter: indexmap::map::Iter<'_, Name, nameres::ScopeItemDef> =
            def_map[scope].declarations.iter();
        let iter = unsafe { transmute(iter) };
        Scope { _def_map: def_map, iter, def: block }
    }
}

pub struct RecDeclarations<'a> {
    path: Vec<Name>,
    stack: Vec<Scope>,
    db: &'a CompilationDB,
}

impl<'a> RecDeclarations<'a> {
    pub(super) fn new(scope: super::Scope, db: &'a CompilationDB) -> RecDeclarations<'a> {
        let (scope_id, def_map) = scope.def_map_and_scope(db);
        RecDeclarations { path: Vec::new(), stack: vec![Scope::new(def_map, scope_id, None)], db }
    }

    /// Creates a path in the current scope with the final segment given by `name`
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
                    ScopeItemDef::BlockId(id) => {
                        if let Some(def_map) = self.db.block_def_map(id) {
                            let entry = def_map.entry_scope();
                            self.stack.push(Scope::new(
                                def_map,
                                entry,
                                Some((name.clone(), ScopeDef::Block(Block { id }))),
                            ))
                        }
                        continue;
                    }
                    ScopeItemDef::ModuleId(id) => ScopeDef::Module(Module { id }),
                    ScopeItemDef::NodeId(id) => ScopeDef::Node(Node { id }),
                    ScopeItemDef::BranchId(id) => ScopeDef::Branch(Branch { id }),
                    ScopeItemDef::VarId(id) => ScopeDef::Variable(Variable { id }),
                    ScopeItemDef::ParamId(id) => ScopeDef::Parameter(Parameter { id }),
                    ScopeItemDef::AliasParamId(id) => ScopeDef::AliasParam(AliasParam { id }),
                    _ => continue,
                };
                return Some((name.clone(), def));
            } else {
                self.path.pop();
                let scope = self.stack.pop()?;
                if let Some(def) = scope.def {
                    return Some(def);
                }
                continue;
            }
        }
    }
}
