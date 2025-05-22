//! Type-related item reference.

use std::sync::Arc;

use hir_def::{
    nameres::{DefMap, PathResolveError},
    BranchId, DisciplineId, Intern, Lookup, NatureAttrId, NatureAttrLoc, NatureId, NatureRef,
    NatureRefKind, NodeId,
};
use syntax::name::{kw, Name};
use syntax::SyntaxNodePtr;

use crate::db::HirTyDB;

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct PathError {
    pub err: PathResolveError,
    pub src: SyntaxNodePtr,
}

/// Resolve nature reference, including parent nature and discipline nature binding
pub fn resolve_nature(
    def_map: &DefMap,
    nature_ref: &NatureRef,
    db: &dyn HirTyDB,
) -> Result<NatureId, PathError> {
    let root_scope = def_map.root_scope();
    let src = nature_ref.src;
    let (nature, name) = match nature_ref.kind {
        NatureRefKind::Nature => {
            return def_map
                .resolve_item_name(root_scope, &nature_ref.name)
                .map_err(|err| PathError { err, src })
        }
        NatureRefKind::DisciplinePotential => {
            let discipline = def_map
                .resolve_item_name(root_scope, &nature_ref.name)
                .map_err(|err| PathError { err, src })?;
            (db.discipline_info(discipline)?.potential, kw::potential)
        }
        NatureRefKind::DisciplineFlow => {
            let discipline = def_map
                .resolve_item_name(root_scope, &nature_ref.name)
                .map_err(|err| PathError { err, src })?;
            (db.discipline_info(discipline)?.flow, kw::flow)
        }
    };

    nature.ok_or_else(|| {
        let err = PathResolveError::NotFoundIn { name, scope: nature_ref.name.clone() };
        PathError { err, src }
    })
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct NatureTy {
    pub units: Option<String>,
    pub parent_nature: Option<NatureId>,
    pub base_nature: NatureId,
    pub ddt_nature: NatureId,
    pub idt_nature: NatureId,
}

impl NatureTy {
    pub fn nature_info_query(
        db: &dyn HirTyDB,
        nature: NatureId,
    ) -> Result<Arc<NatureTy>, PathError> {
        Self::obtain(db, nature, true)
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    pub(crate) fn nature_info_recover(
        db: &dyn HirTyDB,
        _cycle: &salsa::Cycle,
        nature: &NatureId,
    ) -> Result<Arc<NatureTy>, PathError> {
        Self::obtain(db, *nature, false)
    }

    fn obtain(
        db: &dyn HirTyDB,
        nature: NatureId,
        resolve_parent: bool,
    ) -> Result<Arc<NatureTy>, PathError> {
        let data = db.nature_data(nature);
        let loc = nature.lookup(db.upcast());
        let def_map = db.root_def_map(loc.root_file);

        let parent_nature =
            data.parent.as_ref().map(|parent| resolve_nature(&def_map, parent, db)).transpose()?;
        let parent_nature_info = parent_nature
            .and_then(|parent| resolve_parent.then(|| db.nature_info(parent)))
            .transpose()?;
        let base_nature = parent_nature_info.as_ref().map(|parent| parent.base_nature);

        let units = base_nature
            .map(|nature| db.nature_info(nature))
            .transpose()?
            .as_ref()
            .and_then(|base| base.units.clone())
            .or_else(|| data.units.clone());

        let ddt_nature = data
            .ddt_nature
            .as_ref()
            .map(|ddt_nature| resolve_nature(&def_map, ddt_nature, db))
            .transpose()?
            .or_else(|| parent_nature_info.as_ref().map(|parent| parent.ddt_nature));

        let idt_nature = data
            .idt_nature
            .as_ref()
            .map(|idt_nature| resolve_nature(&def_map, idt_nature, db))
            .transpose()?
            .or_else(|| parent_nature_info.as_ref().map(|parent| parent.idt_nature));

        Ok(Arc::new(NatureTy {
            parent_nature,
            units,
            base_nature: base_nature.unwrap_or(nature),
            ddt_nature: ddt_nature.unwrap_or(nature),
            idt_nature: idt_nature.unwrap_or(nature),
        }))
    }

    /// [LRM 3.11.1]
    /// Two natures are compatible if they have the same value for the units attribute
    fn compatible(db: &dyn HirTyDB, nature1: NatureId, nature2: NatureId) -> bool {
        db.nature_info(nature1).unwrap().units == db.nature_info(nature2).unwrap().units
    }

    // fn related(db: &dyn HirTyDB, nature1: NatureId, nature2: NatureId) -> bool {
    //     let nature1_info = db.nature_info(nature1);
    //     let nature2_info = db.nature_info(nature2);
    //     nature1_info.base_nature == nature2_info.base_nature
    // }

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
                let data = db.nature_data(nature);
                if let Some((id, _)) =
                    data.attrs.iter_enumerated().find(|(_, attr)| &attr.name == name)
                {
                    let attr = NatureAttrLoc { nature, id }.intern(db.upcast());
                    return Some(attr);
                }
                let info = db.nature_info(nature).unwrap();
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

impl DisciplineTy {
    pub fn discipline_info_query(
        db: &dyn HirTyDB,
        discipline: DisciplineId,
    ) -> Result<Arc<DisciplineTy>, PathError> {
        let data = db.discipline_data(discipline);
        let loc = discipline.lookup(db.upcast());
        let def_map = db.root_def_map(loc.root_file);

        let flow = data.flow.as_ref().map(|flow| resolve_nature(&def_map, flow, db)).transpose()?;
        let potential =
            data.potential.as_ref().map(|pot| resolve_nature(&def_map, pot, db)).transpose()?;

        Ok(Arc::new(DisciplineTy { flow, potential }))
    }

    /// [LRM 3.11.1]
    /// - A discipline is compatible with itself.
    /// - A natureless discipline is compatible with all other disciplines of the same domain.
    /// - Disciplines with different domain attributes are incompatible.
    /// - Disciplines with incompatible potential natures are incompatible.
    /// - Disciplines with incompatible flow natures are incompatible.
    pub fn compatible(&self, other: DisciplineId, db: &dyn HirTyDB) -> bool {
        let other = db.discipline_info(other).unwrap();
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

    pub fn access(&self, nature: NatureId, db: &dyn HirTyDB) -> Option<DisciplineAccess> {
        let Self { flow, potential } = self;
        if flow.is_some_and(|flow| NatureTy::compatible(db, flow, nature)) {
            Some(DisciplineAccess::Flow)
        } else if potential.is_some_and(|potential| NatureTy::compatible(db, potential, nature)) {
            Some(DisciplineAccess::Potential)
        } else {
            None
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum DisciplineAccess {
    Potential,
    Flow,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct BranchTy {
    pub discipline: DisciplineId,
    pub kind: BranchKind,
}

impl BranchTy {
    pub fn branch_info_query(db: &dyn HirTyDB, branch: BranchId) -> Option<Arc<BranchTy>> {
        let loc = branch.lookup(db.upcast());
        let scope = loc.scope;

        let kind = match &db.branch_data(branch).kind {
            hir_def::BranchKind::PortFlow(port) => {
                let port_flow = scope.resolve_item_path(db.upcast(), port).ok()?;
                BranchKind::PortFlow(port_flow)
            }
            hir_def::BranchKind::NodeGnd(node) => {
                let node = scope.resolve_item_path(db.upcast(), node).ok()?;
                BranchKind::NodeGnd(node)
            }
            hir_def::BranchKind::Nodes(node1, node2) => {
                let node1 = scope.resolve_item_path(db.upcast(), node1).ok()?;
                let node2 = scope.resolve_item_path(db.upcast(), node2).ok()?;
                BranchKind::Nodes(node1, node2)
            }
            hir_def::BranchKind::Missing => return None,
        };

        kind.discipline(db).map(|discipline| Arc::new(BranchTy { discipline, kind }))
    }

    pub fn access(&self, nature: NatureId, db: &dyn HirTyDB) -> Option<DisciplineAccess> {
        db.discipline_info(self.discipline).unwrap().access(nature, db)
    }

    pub fn flow_attr(
        db: &dyn HirTyDB,
        branch: BranchId,
        name: &Name,
    ) -> Option<Result<NatureAttrId, PathResolveError>> {
        let discipline = db.branch_info(branch)?.discipline;
        match db.discipline_info(discipline).unwrap().flow {
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
        match db.discipline_info(discipline).unwrap().potential {
            Some(nature) => Some(NatureTy::lookup_attr(db, nature, name)),
            None => Some(Err(PathResolveError::NotFoundIn {
                name: kw::potential,
                scope: db.discipline_data(discipline).name.clone(),
            })),
        }
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
                // Compatible disciplines have identical behaviors during type checking, so we
                // just use the discipline of the first node here.
                let d1 = db.node_discipline(node1);
                let d2 = db.node_discipline(node2);
                // fast path
                if d1 == d2 {
                    return d1;
                }
                let (d1, d2) = match (d1, d2) {
                    (None, d) | (d, None) => return d,
                    (Some(d1), Some(d2)) => (d1, d2),
                };
                db.discipline_info(d1).unwrap().compatible(d2, db).then_some(d1)
            }
        }
    }
}
