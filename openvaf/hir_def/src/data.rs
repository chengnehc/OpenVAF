//! Item data and queries.

use std::sync::Arc;

use arena::Arena;
use syntax::name::Name;
use typed_index_collections::TiSlice;

use crate::db::HirDefDB;
use crate::item_tree::{BranchKind, DisciplineAttrKind, Domain, FunctionArg, NatureRef};
use crate::{
    AliasParamId, BranchId, DisciplineId, FunctionId, Intern, ItemTree, LocalFunctionArgId,
    LocalNatureAttrId, Lookup, ModuleId, NatureId, NodeId, NodeLoc, ParamId, Type, VarId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NatureData {
    pub name: Name,
    pub parent: Option<NatureRef>,
    pub idt_nature: Option<NatureRef>,
    pub ddt_nature: Option<NatureRef>,
    pub units: Option<String>,
    pub abstol: Option<LocalNatureAttrId>,
    pub attrs: Arena<NatureAttrData>,
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct NatureAttrData {
    pub name: Name,
}

impl NatureData {
    pub fn query(db: &dyn HirDefDB, id: NatureId) -> Arc<NatureData> {
        let loc = id.lookup(db);
        let tree = db.item_tree(loc.root_file);
        let nature = &tree[loc.id];
        let attrs: Arena<_> = nature
            .attrs
            .clone()
            .map(|attr| NatureAttrData { name: tree[attr].name.clone() })
            .collect();

        Arc::new(NatureData {
            name: nature.name.clone(),
            parent: nature.parent.clone(),
            idt_nature: nature.idt_nature.clone().map(|(it, _)| it),
            ddt_nature: nature.ddt_nature.clone().map(|(it, _)| it),
            units: nature.units.clone().map(|(it, _)| it),
            abstol: nature.abstol,
            attrs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisciplineData {
    pub name: Name,
    pub potential: Option<NatureRef>,
    pub flow: Option<NatureRef>,
    pub domain: Option<Domain>,
    pub attrs: Arena<DisciplineAttrData>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisciplineAttrData {
    pub name: Name,
    pub kind: DisciplineAttrKind,
}

impl DisciplineData {
    pub fn query(db: &dyn HirDefDB, id: DisciplineId) -> Arc<DisciplineData> {
        let loc = id.lookup(db);
        let tree = &loc.item_tree(db);
        let discipline = &tree[loc.id];
        let attrs: Arena<_> = discipline
            .attrs
            .clone()
            .map(|attr| {
                let attr = &tree[attr];
                DisciplineAttrData { name: attr.name.clone(), kind: attr.kind }
            })
            .collect();

        Arc::new(DisciplineData {
            name: discipline.name.clone(),
            potential: discipline.potential.clone().map(|(pot, _)| pot),
            flow: discipline.flow.clone().map(|(flow, _)| flow),
            domain: discipline.domain.map(|(domain, _)| domain),
            attrs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleData {
    pub name: Name,
    pub ports: Vec<NodeId>,
    pub internal_nodes: Vec<NodeId>,
}

impl ModuleData {
    pub fn query(db: &dyn HirDefDB, module: ModuleId) -> Arc<ModuleData> {
        let loc = module.lookup(db);
        let tree = loc.item_tree(db);
        let num_ports = tree[loc.id].num_ports;
        let num_nodes = tree[loc.id].nodes.len() as u32;
        let ports = (0..num_ports).map(|id| NodeLoc { module, id: id.into() }.intern(db)).collect();
        let internal_nodes =
            (num_ports..num_nodes).map(|id| NodeLoc { module, id: id.into() }.intern(db)).collect();

        Arc::new(ModuleData { name: tree[loc.id].name.clone(), ports, internal_nodes })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeData {
    pub name: Name,
    pub discipline: Option<Name>,
    pub is_input: bool,
    pub is_output: bool,
    pub is_gnd: bool,
}

impl NodeData {
    pub fn query(db: &dyn HirDefDB, id: NodeId) -> Arc<NodeData> {
        let loc = id.lookup(db);
        let module = loc.module.lookup(db);
        let tree = module.item_tree(db);
        let node = &tree[module.id].nodes[loc.id];
        let (is_input, is_output) = node.direction(&tree);

        Arc::new(NodeData {
            name: node.name.clone(),
            discipline: node.discipline(&tree),
            is_input,
            is_output,
            is_gnd: node.is_gnd(&tree),
        })
    }

    #[inline]
    pub fn is_port(&self) -> bool {
        self.is_input | self.is_output
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchData {
    pub name: Name,
    pub kind: BranchKind,
}

impl BranchData {
    pub fn query(db: &dyn HirDefDB, id: BranchId) -> Arc<BranchData> {
        let loc = id.lookup(db);
        let branch = &loc.item_tree(db)[loc.id];

        Arc::new(BranchData { name: branch.name.clone(), kind: branch.kind.clone() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarData {
    pub name: Name,
    pub ty: Type,
}

impl VarData {
    pub fn query(db: &dyn HirDefDB, id: VarId) -> Arc<VarData> {
        let loc = id.lookup(db);
        let var = &loc.item_tree(db)[loc.id];

        Arc::new(VarData { name: var.name.clone(), ty: var.ty.clone() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamData {
    pub name: Name,
    pub ty: Option<Type>,
}

impl ParamData {
    pub fn query(db: &dyn HirDefDB, id: ParamId) -> Arc<ParamData> {
        let loc = id.lookup(db);
        let param = &loc.item_tree(db)[loc.id];

        Arc::new(ParamData { name: param.name.clone(), ty: param.ty.clone() })
    }
}

#[derive(Debug, Eq, PartialEq, Clone)]
pub struct AliasParamData {
    pub name: Name,
    pub param_ref: Name,
}

impl AliasParamData {
    pub fn query(db: &dyn HirDefDB, id: AliasParamId) -> Arc<AliasParamData> {
        let loc = id.lookup(db);
        let param = &loc.item_tree(db)[loc.id];

        Arc::new(AliasParamData { name: param.name.clone(), param_ref: param.param_ref.clone() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionData {
    pub name: Name,
    pub args: Box<TiSlice<LocalFunctionArgId, FunctionArgData>>,
    pub return_ty: Type,
}

impl FunctionData {
    pub fn query(db: &dyn HirDefDB, id: FunctionId) -> Arc<FunctionData> {
        let loc = id.lookup(db);
        let tree = loc.item_tree(db);
        let fun = &tree[loc.id];
        let args = fun.args.iter().map(|arg| FunctionArgData::new(arg, &tree)).collect();

        Arc::new(FunctionData { name: fun.name.clone(), return_ty: fun.ty.clone(), args })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionArgData {
    pub name: Name,
    pub ty: Type,
    pub is_input: bool,
    pub is_output: bool,
}

impl FunctionArgData {
    fn new(arg: &FunctionArg, tree: &ItemTree) -> FunctionArgData {
        FunctionArgData {
            name: arg.name.clone(),
            ty: arg.ty(tree),
            is_input: arg.is_input,
            is_output: arg.is_output,
        }
    }
}
