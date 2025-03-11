//use stdx::impl_display;

use basedb::{AstId, ErasedAstId, FileId};
use hir_def::nameres::PathResolveError;
use hir_def::{BranchId, DisciplineId, LocalDisciplineAttrId, LocalNatureAttrId, NatureId, NodeId};
use syntax::name::Name;
use syntax::{ast, SyntaxNodePtr};

use crate::db::HirTyDB;

use super::TypeValidator;

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct DuplicateItem<Item, Def> {
    pub src: Def,
    pub first: Item,
    pub subsequent: Vec<Item>,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub enum TypeDiagnostic {
    PathError { err: PathResolveError, src: SyntaxNodePtr },
    DuplicateNatureAttr(DuplicateItem<LocalNatureAttrId, NatureId>),
    DuplicateDisciplineAttr(DuplicateItem<LocalDisciplineAttrId, DisciplineId>),
    PortWithoutDirection { decl: ErasedAstId, name: Name },
    NodeWithoutDiscipline { decl: ErasedAstId, name: Name },
    MultipleDirections(DuplicateItem<AstId<ast::PortDecl>, NodeId>),
    MultipleDisciplines(DuplicateItem<ErasedAstId, NodeId>),
    MultipleGnds(DuplicateItem<ErasedAstId, NodeId>),
    ExpectedPort { node: NodeId, src: ErasedAstId },
    IncompatibleBranch { branch: BranchId, node1: NodeId, node2: NodeId },
}

impl TypeDiagnostic {
    pub fn validate_and_collect(db: &dyn HirTyDB, root_file: FileId) -> Vec<TypeDiagnostic> {
        let mut diagnostics = Vec::new();
        let def_map = &db.root_def_map(root_file);
        let tree = &db.item_tree(root_file);
        TypeValidator { db, def_map, tree, root_file, diagnostics: &mut diagnostics }.validate();

        diagnostics
    }
}
