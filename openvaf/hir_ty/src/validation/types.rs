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
    DuplicateDisciplineAttr(DuplicateItem<LocalDisciplineAttrId, DisciplineId>),
    DuplicateNatureAttr(DuplicateItem<LocalNatureAttrId, NatureId>),
    MultipleDirections(DuplicateItem<AstId<ast::PortDecl>, NodeId>),
    MultipleDisciplines(DuplicateItem<ErasedAstId, NodeId>),
    MultipleGnds(DuplicateItem<ErasedAstId, NodeId>),
    PortWithoutDirection { decl: ErasedAstId, name: Name },
    NodeWithoutDiscipline { decl: ErasedAstId, name: Name },
    ExpectedPort { node: NodeId, src: ErasedAstId },
    IncompatibleBranch { branch: BranchId, node1: NodeId, node2: NodeId },
}

/*
use TypeDiagnostic::*;
impl_display! {
    match TypeDiagnostic {
        PathError{..} => "";
        _ => "";
    }
}
*/

impl TypeDiagnostic {
    pub fn collect(db: &dyn HirTyDB, root_file: FileId) -> Vec<TypeDiagnostic> {
        let mut res = Vec::new();
        let def_map = db.root_def_map(root_file);
        let tree = db.item_tree(root_file);
        TypeValidator { db, dst: &mut res, def_map: &def_map, tree: &tree, root_file }.validate();

        res
    }
}
