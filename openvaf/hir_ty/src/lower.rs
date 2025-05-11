use std::sync::Arc;

use hir_def::{
    nameres::{DefMap, PathResolveError},
    BranchId, DisciplineId, Intern, Lookup, NatureAttrId, NatureAttrLoc, NatureId, NatureRef,
    NatureRefKind, NodeId,
};
use syntax::name::{kw, Name};

use crate::db::HirTyDB;

/// Look up the base nature of a nature, or the nature reference of a discipline.
pub fn lookup_nature(
    def_map: &DefMap,
    nature_ref: &NatureRef,
    db: &dyn HirTyDB,
) -> Result<NatureId, PathResolveError> {
    let root_scope = def_map.root_scope();
    let (nature, attr) = match nature_ref.kind {
        NatureRefKind::Nature => return def_map.resolve_item_name(root_scope, &nature_ref.name),
        NatureRefKind::DisciplinePotential => {
            let discipline = def_map.resolve_item_name(root_scope, &nature_ref.name)?;
            (db.discipline_info(discipline).potential, kw::potential)
        }
        NatureRefKind::DisciplineFlow => {
            let discipline = def_map.resolve_item_name(root_scope, &nature_ref.name)?;
            (db.discipline_info(discipline).flow, kw::flow)
        }
    };

    nature
        .ok_or_else(|| PathResolveError::NotFoundIn { name: attr, scope: nature_ref.name.clone() })
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct NatureTy {
    pub parent_nature: Option<NatureId>,
    pub base_nature: NatureId,
    pub ddt_nature: NatureId,
    pub idt_nature: NatureId,
    pub units: Option<String>,
}

impl NatureTy {
    pub fn nature_info_query(db: &dyn HirTyDB, nature: NatureId) -> Arc<NatureTy> {
        Self::obtain(db, nature, true)
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn nature_info_recover(
        db: &dyn HirTyDB,
        _cycle: &salsa::Cycle,
        nature: &NatureId,
    ) -> Arc<NatureTy> {
        Self::obtain(db, *nature, false)
    }

    fn obtain(db: &dyn HirTyDB, nature: NatureId, resolve_parent: bool) -> Arc<NatureTy> {
        let data = db.nature_data(nature);
        let loc = nature.lookup(db.upcast());
        let def_map = db.root_def_map(loc.root_file);

        let parent_nature =
            data.parent.as_ref().and_then(|parent| lookup_nature(&def_map, parent, db).ok());
        let parent_nature_info =
            parent_nature.and_then(|parent| resolve_parent.then(|| db.nature_info(parent)));
        let base_nature_info = parent_nature_info.as_ref().map(|parent| parent.base_nature);

        let ddt_nature = data
            .ddt_nature
            .as_ref()
            .and_then(|ddt_nature| lookup_nature(&def_map, ddt_nature, db).ok())
            .or_else(|| parent_nature_info.as_ref().map(|parent| parent.ddt_nature))
            .unwrap_or(nature);

        let idt_nature = data
            .idt_nature
            .as_ref()
            .and_then(|idt_nature| lookup_nature(&def_map, idt_nature, db).ok())
            .or_else(|| parent_nature_info.as_ref().map(|parent| parent.idt_nature))
            .unwrap_or(nature);

        let units = base_nature_info
            .and_then(|nature| db.nature_info(nature).units.clone())
            .or_else(|| data.units.clone());

        Arc::new(NatureTy {
            parent_nature,
            base_nature: base_nature_info.unwrap_or(nature),
            ddt_nature,
            idt_nature,
            units,
        })
    }

    /// [LRM 3.11.1]
    ///
    /// - Self Rule: A nature is compatible with itself.
    /// - Non-Existent Binding Rule: A nature is compatible with a non-existent discipline binding.
    /// - Base Nature Rule: A derived nature is compatible with its base nature.
    /// - Derived Nature Rule: Two natures are compatible if they are derived from the same base nature.
    /// - [✓] Units Value Rule: Two natures are compatible if they have the same value for the units attribute
    fn compatible(db: &dyn HirTyDB, nature1: NatureId, nature2: NatureId) -> bool {
        let nature1_info = db.nature_info(nature1);
        let nature2_info = db.nature_info(nature2);
        nature1_info.units == nature2_info.units
    }

    /* JW: not used.
        fn related(db: &dyn HirTyDB, nature1: NatureId, nature2: NatureId) -> bool {
            let nature1_info = db.nature_info(nature1);
            let nature2_info = db.nature_info(nature2);
            nature1_info.base_nature == nature2_info.base_nature
        }
    */

    fn lookup_attr(
        db: &dyn HirTyDB,
        nature: NatureId,
        name: &Name,
    ) -> Result<NatureAttrId, PathResolveError> {
        fn lookup_attr_inner(
            db: &dyn HirTyDB,
            mut nature: NatureId,
            name: &Name,
        ) -> Option<NatureAttrId> {
            loop {
                if let Some((attr, _)) = db
                    .nature_data(nature)
                    .attrs
                    .iter_enumerated()
                    .find(|(_, attr)| &attr.name == name)
                {
                    return Some(NatureAttrLoc { nature, id: attr }.intern(db.upcast()));
                }
                let info = db.nature_info(nature);
                if info.base_nature == nature {
                    return None;
                }
                nature = info.parent_nature?;
            }
        }
        lookup_attr_inner(db, nature, name).ok_or_else(|| PathResolveError::NotFoundIn {
            name: name.clone(),
            scope: db.nature_data(nature).name.clone(),
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct DisciplineTy {
    pub flow: Option<NatureId>,
    pub potential: Option<NatureId>,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum DisciplineAccess {
    Potential,
    Flow,
}

impl DisciplineTy {
    pub fn discipline_info_query(db: &dyn HirTyDB, discipline: DisciplineId) -> Arc<DisciplineTy> {
        let data = db.discipline_data(discipline);
        let def_map = db.root_def_map(discipline.lookup(db.upcast()).root_file);
        Arc::new(DisciplineTy {
            flow: data.flow.as_ref().and_then(|flow| lookup_nature(&def_map, flow, db).ok()),
            potential: data
                .potential
                .as_ref()
                .and_then(|potential| lookup_nature(&def_map, potential, db).ok()),
        })
    }

    pub fn access(&self, nature: NatureId, db: &dyn HirTyDB) -> Option<DisciplineAccess> {
        if self.flow.is_some_and(|flow| NatureTy::compatible(db, flow, nature)) {
            Some(DisciplineAccess::Flow)
        } else if self
            .potential
            .is_some_and(|potential| NatureTy::compatible(db, potential, nature))
        {
            Some(DisciplineAccess::Potential)
        } else {
            None
        }
    }

    /// [LRM 3.11.1]
    ///
    /// - Self Rule: A discipline is compatible with itself.
    /// - Natureless Discipline Rule: A natureless discipline is compatible with all other disciplines of the
    ///   same domain.
    /// - Domain Incompatibility Rule: Disciplines with different domain attributes are incompatible.
    /// - Potential Incompatibility Rule: Disciplines with incompatible potential natures are incompatible.
    /// - Flow Incompatibility Rule: Disciplines with incompatible flow natures are incompatible.
    pub fn compatible(&self, other: DisciplineId, db: &dyn HirTyDB) -> bool {
        let other = db.discipline_info(other);
        match (self.flow, other.flow) {
            (Some(flow1), Some(flow2)) => {
                if !NatureTy::compatible(db, flow1, flow2) {
                    return false;
                }
            }
            (None, None) => (),
            _ => return false,
        }
        match (self.potential, other.potential) {
            (Some(pot1), Some(pot2)) => {
                if !NatureTy::compatible(db, pot1, pot2) {
                    return false;
                }
            }
            (None, None) => (),
            _ => return false,
        }

        true
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum BranchKind {
    PortFlow(NodeId),
    NodeGnd(NodeId),
    Nodes(NodeId, NodeId),
}

impl BranchKind {
    pub fn discipline(&self, db: &dyn HirTyDB) -> Option<DisciplineId> {
        match *self {
            BranchKind::PortFlow(node) | BranchKind::NodeGnd(node) => db.node_discipline(node),
            BranchKind::Nodes(node1, node2) => {
                // Standard dictates that the disciplines of the two nodes need to be compatible.
                // Compatible disciplines behave identical during type checking, so we just use
                // the discipline of the first node here.
                let discipline1 = db.node_discipline(node1);
                let discipline2 = db.node_discipline(node2);
                // fast path
                if discipline1 == discipline2 {
                    return discipline1;
                }
                let (discipline1, discipline2) = match (discipline1, discipline2) {
                    (None, res) | (res, None) => return res,
                    (Some(d1), Some(d2)) => (d1, d2),
                };
                db.discipline_info(discipline1).compatible(discipline2, db).then_some(discipline1)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BranchTy {
    pub discipline: DisciplineId,
    pub kind: BranchKind,
}

impl BranchTy {
    pub fn branch_info_query(db: &dyn HirTyDB, branch: BranchId) -> Option<Arc<BranchTy>> {
        let kind = &db.branch_data(branch).kind;
        let scope = branch.lookup(db.upcast()).scope;
        let kind = match kind {
            hir_def::BranchKind::PortFlow(port) => {
                BranchKind::PortFlow(scope.resolve_item_path(db.upcast(), port).ok()?)
            }
            hir_def::BranchKind::NodeGnd(node) => {
                BranchKind::NodeGnd(scope.resolve_item_path(db.upcast(), node).ok()?)
            }
            hir_def::BranchKind::Nodes(node1, node2) => {
                let node1 = scope.resolve_item_path(db.upcast(), node1).ok()?;
                let node2 = scope.resolve_item_path(db.upcast(), node2).ok()?;
                BranchKind::Nodes(node1, node2)
            }
            hir_def::BranchKind::Missing => return None,
        };

        Some(Arc::new(BranchTy { discipline: kind.discipline(db)?, kind }))
    }

    pub fn access(&self, nature: NatureId, db: &dyn HirTyDB) -> Option<DisciplineAccess> {
        db.discipline_info(self.discipline).access(nature, db)
    }

    pub fn flow_attr(
        db: &dyn HirTyDB,
        branch: BranchId,
        name: &Name,
    ) -> Option<Result<NatureAttrId, PathResolveError>> {
        let discipline = db.branch_info(branch)?.discipline;
        match db.discipline_info(discipline).flow {
            Some(nature) => Some(NatureTy::lookup_attr(db, nature, name)),
            None => Some(Err(PathResolveError::NotFoundIn {
                name: kw::flow,
                scope: db.discipline_data(discipline).name.clone(),
            })),
        }
    }

    pub fn potential_attr(
        db: &dyn HirTyDB,
        branch: BranchId,
        name: &Name,
    ) -> Option<Result<NatureAttrId, PathResolveError>> {
        let discipline = db.branch_info(branch)?.discipline;
        match db.discipline_info(discipline).potential {
            Some(nature) => Some(NatureTy::lookup_attr(db, nature, name)),
            None => Some(Err(PathResolveError::NotFoundIn {
                name: kw::potential,
                scope: db.discipline_data(discipline).name.clone(),
            })),
        }
    }
}
