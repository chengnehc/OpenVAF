use std::iter;

use hir_def::{
    nameres::{PathResolveError, ScopeItemDef},
    AliasParamId, BranchId, BranchLoc, DisciplineId, ModuleId, ModuleLoc, NatureId, NodeId,
    NodeTypeDecl, Path, Scope,
};
use syntax::{ast::ArgListOwner, AstNode, SyntaxNodePtr};
use typed_index_collections::TiSlice;

use super::diagnostics::DuplicateItem;
use super::*;

impl TypeValidator<'_> {
    pub(super) fn validate(mut self) -> Vec<TypeDiagnostic> {
        let root = &self.def_map[self.def_map.root_scope()];
        for def in root.declarations.values() {
            match *def {
                ScopeItemDef::NatureId(nature) => self.verify_nature(nature),
                ScopeItemDef::DisciplineId(discipline) => self.verify_discipline(discipline),
                ScopeItemDef::ModuleId(module) => self.verify_module(module),
                _ => (),
            }
        }
        self.diagnostics
    }

    fn verify_nature(&mut self, nature: NatureId) {
        // let info = self.db.nature_info(nature);
        let data = self.db.nature_data(nature);
        self.verify_unique_attr(&data.attrs, nature, TypeDiagnostic::DuplicateNatureAttr);
    }

    // TODO check natures/discipline (~dspom/OpenVAF#1)
    fn verify_discipline(&mut self, discipline: DisciplineId) {
        // let info = self.db.discipline_info(discipline);
        let data = self.db.discipline_data(discipline);
        self.verify_unique_attr(&data.attrs, discipline, TypeDiagnostic::DuplicateDisciplineAttr);
    }

    fn verify_unique_attr<Attr: From<usize> + PartialEq, Def: Copy>(
        &mut self,
        attrs: &TiSlice<Attr, impl PartialEq>,
        def: Def,
        wrap_err: impl Fn(DuplicateItem<Attr, Def>) -> TypeDiagnostic,
    ) {
        // This is quadratic (actually its n(n+1)/2). But disciplines and nature usually only have very few (below 5)
        // attributes so this is probably faster than allocating a HashMap. If this ever becomes a
        // problem just use a HashMap instead
        for (id, attr) in attrs.iter_enumerated() {
            let mut duplicates =
                attrs.iter_enumerated().filter_map(
                    |it| {
                        if it.1 == attr {
                            Some(it.0)
                        } else {
                            None
                        }
                    },
                );
            if duplicates.next().unwrap() != id {
                continue;
            }
            let duplicates: Vec<_> = duplicates.collect();
            if !duplicates.is_empty() {
                let err = DuplicateItem { src: def, first: id, subsequent: duplicates };
                self.report(wrap_err(err))
            }
        }
    }

    fn verify_module(&mut self, module: ModuleId) {
        let loc = module.lookup(self.db.upcast());
        let scope = loc.scope.local_id;
        for item in self.def_map[scope].declarations.values() {
            match item {
                ScopeItemDef::NodeId(node) => self.verify_node(*node, loc),
                ScopeItemDef::BranchId(branch) => self.verify_branch(*branch),
                ScopeItemDef::AliasParamId(alias) => self.verify_alias(*alias),
                _ => (),
            }
        }
    }

    fn verify_node(&mut self, node: NodeId, module: ModuleLoc) {
        let loc = node.lookup(self.db.upcast());
        let node_ = &self.item_tree[module.id].nodes[loc.id];
        if node_.decls.is_empty() {
            self.report(TypeDiagnostic::PortWithoutDirection {
                decl: node_.ast_id,
                name: node_.name.clone(),
            });
            self.report(TypeDiagnostic::NodeWithoutDiscipline {
                decl: node_.ast_id,
                name: node_.name.clone(),
            });
            return; // Do not print other diagnostics here would just lead to duplications
        }
        let mut directions = node_.decls.iter().filter_map(|decl| {
            if let NodeTypeDecl::Port(p) = decl {
                Some(self.item_tree[*p].ast_id)
            } else {
                None
            }
        });
        if let Some(first) = directions.next() {
            let duplicates: Vec<_> = directions.collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleDirections(DuplicateItem {
                    src: node,
                    first,
                    subsequent: duplicates,
                }))
            }
        } else if node_.decls[0].ast_id(self.item_tree) != node_.ast_id {
            self.report(TypeDiagnostic::PortWithoutDirection {
                decl: node_.ast_id,
                name: node_.name.clone(),
            })
        }

        let mut disciplines = node_.decls.iter().filter_map(|it| {
            it.discipline(self.item_tree).as_ref().map(|discipline| (it, discipline))
        });

        if let Some((decl, discipline)) = disciplines.next() {
            for (decl, discipline) in iter::once((decl, discipline)).chain(disciplines.clone()) {
                if let Err(err) = self
                    .def_map
                    .resolve_item_name::<DisciplineId>(self.def_map.root_scope(), discipline)
                {
                    self.report(TypeDiagnostic::PathError {
                        err,
                        src: SyntaxNodePtr::new(
                            decl.discipline_source(self.db.upcast(), self.root_file)
                                .unwrap()
                                .syntax(),
                        ),
                    })
                }
            }

            let duplicates: Vec<_> =
                disciplines.map(|(decl, _)| decl.ast_id(self.item_tree)).collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleDisciplines(DuplicateItem {
                    src: node,
                    first: decl.ast_id(self.item_tree),
                    subsequent: duplicates,
                }))
            }
        } else {
            self.report(TypeDiagnostic::NodeWithoutDiscipline {
                decl: node_.ast_id,
                name: node_.name.clone(),
            });
        }

        let mut gnd_declarations = node_.decls.iter().filter(|it| it.is_gnd(self.item_tree));

        if let Some(first) = gnd_declarations.next() {
            let duplicates: Vec<_> = gnd_declarations.map(|it| it.ast_id(self.item_tree)).collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleDisciplines(DuplicateItem {
                    src: node,
                    first: first.ast_id(self.item_tree),
                    subsequent: duplicates,
                }))
            }
        }

        // TODO(JW): multiple grounds
    }

    fn verify_branch(&mut self, branch_: BranchId) {
        let branch_data = self.db.branch_data(branch_);
        let kind = &branch_data.kind;
        let branch = branch_.lookup(self.db.upcast());
        let scope = branch.scope;
        match kind {
            hir_def::BranchKind::Missing => (),
            hir_def::BranchKind::PortFlow(port) => {
                if let Some(node) = self.resolve_node(port, scope, &branch) {
                    let node_ = self.db.node_data(node);
                    if !node_.is_input && !node_.is_output {
                        let src = branch.ast_id(self.db.upcast()).into();
                        self.report(TypeDiagnostic::ExpectedPort { node, src });
                    }
                }
            }
            hir_def::BranchKind::NodeGnd(node) => {
                self.resolve_node(node, scope, &branch);
            }
            hir_def::BranchKind::Nodes(node1, node2) => {
                let node1 = self.resolve_node(node1, scope, &branch);
                let node2 = self.resolve_node(node2, scope, &branch);
                let (Some(node1), Some(node2)) = (node1, node2) else { return };

                let discipline1 = self.db.node_discipline(node1);
                let discipline2 = self.db.node_discipline(node2);
                // fast path
                if discipline1 == discipline2 {
                    return;
                }
                let (Some(d1), Some(d2)) = (discipline1, discipline2) else { return };
                if !self.db.discipline_info(d1).compatible(d2, self.db) {
                    self.report(TypeDiagnostic::IncompatibleBranch {
                        branch: branch_,
                        node1,
                        node2,
                    })
                }
            }
        };
    }

    fn resolve_node(&mut self, node: &Path, scope: Scope, branch: &BranchLoc) -> Option<NodeId> {
        let node = scope.resolve_item_path::<NodeId>(self.db.upcast(), node);
        match node {
            Ok(node) => Some(node),
            Err(err) => {
                let src = SyntaxNodePtr::new(
                    branch
                        .source(self.db.upcast())
                        .arg_list()
                        .unwrap()
                        .args()
                        .next()
                        .unwrap()
                        .syntax(),
                );
                self.report(TypeDiagnostic::PathError { err, src });
                None
            }
        }
    }

    fn verify_alias(&mut self, alias: AliasParamId) {
        if self.db.resolve_alias(alias).is_none() {
            let loc = alias.lookup(self.db.upcast());
            let data = self.db.aliasparam_data(alias);
            if let Some(path) = data.src.as_ref() {
                match loc.scope.resolve_path(self.db.upcast(), path) {
                    // TODO: better errors for cycles
                    Ok(found) => {
                        let src = SyntaxNodePtr::new(
                            loc.source(self.db.upcast()).src().unwrap().syntax(),
                        );
                        self.report(TypeDiagnostic::PathError {
                            err: PathResolveError::ExpectedItemKind {
                                name: path.segments.last().unwrap().clone(),
                                expected: "parameter",
                                found,
                            },
                            src,
                        })
                    }
                    Err(err) => {
                        let src = SyntaxNodePtr::new(
                            loc.source(self.db.upcast()).src().unwrap().syntax(),
                        );
                        self.report(TypeDiagnostic::PathError { err, src })
                    }
                }
            }
        }
    }

    #[inline]
    fn report(&mut self, diag: impl Into<TypeDiagnostic>) {
        self.diagnostics.push(diag.into())
    }
}
