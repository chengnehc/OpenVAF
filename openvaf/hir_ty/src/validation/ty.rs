use std::iter;

use hir_def::{
    nameres::{ScopeId, ScopeItem},
    AliasParamId, BranchId, DisciplineId, FunctionId, ModuleId, ModuleLoc, NatureId, NodeId,
    NodeTypeDecl, Path,
};
use syntax::{ast, AstNode, SyntaxNodePtr};
use typed_index_collections::TiSlice;

use crate::lower::PathError;

use super::diagnostics::DuplicateItem;
use super::*;

impl TypeValidator<'_> {
    pub(super) fn validate(mut self) -> Vec<TypeDiagnostic> {
        let root = &self.def_map[self.def_map.root_scope()];
        for def in root.decls().values() {
            match *def {
                ScopeItem::NatureId(nature) => self.verify_nature(nature),
                ScopeItem::DisciplineId(discipline) => self.verify_discipline(discipline),
                ScopeItem::ModuleId(module) => self.verify_module(module),
                _ => (),
            }
        }
        self.diagnostics
    }

    fn verify_nature(&mut self, nature: NatureId) {
        if let Err(err) = self.db.nature_info(nature) {
            self.report(TypeDiagnostic::PathResolveError(err));
        }
        let data = self.db.nature_data(nature);
        self.verify_unique_attr(&data.attrs, nature, TypeDiagnostic::DuplicateNatureAttr);
    }

    fn verify_discipline(&mut self, discipline: DisciplineId) {
        if let Err(err) = self.db.discipline_info(discipline) {
            self.report(TypeDiagnostic::PathResolveError(err));
        }
        let data = self.db.discipline_data(discipline);
        self.verify_unique_attr(&data.attrs, discipline, TypeDiagnostic::DuplicateDisciplineAttr);
    }

    fn verify_unique_attr<Attr: From<usize> + PartialEq, Def: Copy>(
        &mut self,
        attrs: &TiSlice<Attr, impl PartialEq>,
        def: Def,
        wrap_err: impl Fn(DuplicateItem<Attr, Def>) -> TypeDiagnostic,
    ) {
        // This is quadratic (actually its n(n+1)/2). But disciplines and nature usually only have
        // very few (below 5) attributes so this is probably faster than allocating a HashMap.
        // If this ever becomes a problem just use a HashMap instead
        for (idx, attr) in attrs.iter_enumerated() {
            let mut duplicates = attrs.iter_enumerated().filter(|it| it.1 == attr).map(|it| it.0);
            if duplicates.next().unwrap() != idx {
                continue;
            }
            let duplicates: Vec<_> = duplicates.collect();
            if !duplicates.is_empty() {
                let err = DuplicateItem { src: def, first: idx, subsequent: duplicates };
                self.report(wrap_err(err))
            }
        }
    }

    fn verify_module(&mut self, id: ModuleId) {
        let module = id.lookup(self.db.upcast());
        for item in self.def_map[module.scope.id].decls().values() {
            match item {
                ScopeItem::NodeId(node) => self.verify_node(*node, module),
                ScopeItem::BranchId(branch) => self.verify_branch(*branch),
                ScopeItem::AliasParamId(alias) => self.verify_alias(*alias),
                ScopeItem::FunctionId(func) => self.verify_function(*func),
                _ => (),
            }
        }
    }

    fn verify_node(&mut self, id: NodeId, module: ModuleLoc) {
        let tree = self.item_tree;
        let node = id.lookup(self.db.upcast());
        let node = &tree[module.id].nodes[node.id];
        if node.decls.is_empty() {
            self.report(TypeDiagnostic::PortWithoutDirection {
                decl: node.ast_id,
                name: node.name.clone(),
            });
            self.report(TypeDiagnostic::NodeWithoutDiscipline {
                decl: node.ast_id,
                name: node.name.clone(),
            });
            return; // Do not print other diagnostics here would just lead to duplications
        }

        /* port direction */
        let mut direction_decls = node.decls.iter().filter_map(|decl| {
            if let NodeTypeDecl::Port(p) = decl {
                Some(tree[*p].ast_id.erased())
            } else {
                None
            }
        });
        if let Some(first) = direction_decls.next() {
            let duplicates: Vec<_> = direction_decls.collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleDirections(DuplicateItem {
                    src: id,
                    first,
                    subsequent: duplicates,
                }))
            }
        } else if node.decls[0].ast_id(tree) != node.ast_id {
            self.report(TypeDiagnostic::PortWithoutDirection {
                decl: node.ast_id,
                name: node.name.clone(),
            })
        }

        /* node discipline */
        let mut disciplines =
            node.decls.iter().filter_map(|it| it.discipline(tree).map(|name| (it, name)));
        if let Some((first, name)) = disciplines.next() {
            for (decl, name) in iter::once((first, name)).chain(disciplines.clone()) {
                if let Err(err) =
                    self.def_map.resolve_item_name::<DisciplineId>(self.def_map.root_scope(), name)
                {
                    let src = decl.discipline_source(self.db.upcast(), self.root_file);
                    self.report(TypeDiagnostic::PathResolveError(PathError { err, src }))
                }
            }
            let duplicates: Vec<_> = disciplines.map(|(decl, _)| decl.ast_id(tree)).collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleDisciplines(DuplicateItem {
                    src: id,
                    first: first.ast_id(tree),
                    subsequent: duplicates,
                }))
            }
        } else {
            self.report(TypeDiagnostic::NodeWithoutDiscipline {
                decl: node.ast_id,
                name: node.name.clone(),
            });
        }

        /* ground net type */
        let mut gnd_decls = node.decls.iter().filter(|decl| decl.is_gnd(tree));
        if let Some(first) = gnd_decls.next() {
            let duplicates: Vec<_> = gnd_decls.map(|decl| decl.ast_id(tree)).collect();
            if !duplicates.is_empty() {
                self.report(TypeDiagnostic::MultipleGnds(DuplicateItem {
                    src: id,
                    first: first.ast_id(tree),
                    subsequent: duplicates,
                }))
            }
        }
    }

    fn verify_branch(&mut self, id: BranchId) {
        let db = self.db.upcast();
        let branch = id.lookup(db);
        let scope = branch.scope;
        let src = branch.source(db);
        let ast_id = branch.ast_id(db);
        match &self.db.branch_data(id).kind {
            hir_def::BranchKind::Missing => (),
            hir_def::BranchKind::NodeGnd(node) => {
                self.resolve_node(node, scope, src.first_node().unwrap());
            }
            hir_def::BranchKind::PortFlow(port) => {
                if let Some(node) = self.resolve_node(port, scope, src.first_node().unwrap()) {
                    if !self.db.node_data(node).is_port() {
                        self.report(TypeDiagnostic::ExpectedPort { node, branch: ast_id });
                    }
                }
            }
            hir_def::BranchKind::Nodes(node1, node2) => {
                let node1 = self.resolve_node(node1, scope, src.first_node().unwrap());
                let node2 = self.resolve_node(node2, scope, src.second_node().unwrap());

                let (Some(node1), Some(node2)) = (node1, node2) else { return };
                let d1 = self.db.node_discipline(node1);
                let d2 = self.db.node_discipline(node2);
                if d1 == d2 {
                    return;
                }
                let (Some(d1), Some(d2)) = (d1, d2) else { return };
                if !self.db.discipline_info(d1).unwrap().compatible(d2, self.db) {
                    self.report(TypeDiagnostic::IncompatibleBranch { branch: id, node1, node2 })
                }
            }
        };
    }

    fn resolve_node(&mut self, path: &Path, scope: ScopeId, src: ast::Expr) -> Option<NodeId> {
        scope
            .resolve_item_path::<NodeId>(self.db.upcast(), path)
            .map_err(|err| {
                let src = SyntaxNodePtr::new(src.syntax());
                self.report(TypeDiagnostic::PathResolveError(PathError { err, src }));
            })
            .ok()
    }

    // TODO: better errors for cycles
    fn verify_alias(&mut self, id: AliasParamId) {
        if let Err(err) = self.db.resolve_alias(id) {
            let db = self.db.upcast();
            let alias = id.lookup(db);
            let node = alias.source(db).param_ref().unwrap();
            let src = SyntaxNodePtr::new(node.syntax());
            self.report(TypeDiagnostic::PathResolveError(PathError { err, src }));
        }
    }

    fn verify_function(&mut self, id: FunctionId) {
        let func = id.lookup(self.db);
        let args = &func.item_tree(self.db)[func.id].args;
        for arg in args {
            let mut vars = arg.var_binds.iter().map(|var| self.item_tree[*var].ast_id);
            if let Some(first) = vars.next() {
                let duplicates: Vec<_> = vars.collect();
                if !duplicates.is_empty() {
                    self.report(TypeDiagnostic::MultipleFuncArgBind(DuplicateItem {
                        src: arg.name.clone(),
                        first,
                        subsequent: duplicates,
                    }))
                }
            } else {
                self.report(TypeDiagnostic::FuncArgWithoutVarBind {
                    decl: arg.ast_id,
                    name: arg.name.clone(),
                });
            }
        }
    }

    #[inline]
    fn report(&mut self, diag: impl Into<TypeDiagnostic>) {
        self.diagnostics.push(diag.into())
    }
}
