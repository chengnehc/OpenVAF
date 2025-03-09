use std::sync::Arc;
use stdx::Upcast;

use basedb::{BaseDB, FileId};

use crate::body::{Body, BodySourceMap, ParamExprs};
use crate::data::{
    AliasParamData, BranchData, DisciplineData, FunctionData, ModuleData, NatureData, NodeData,
    ParamData, VarData,
};
use crate::item_tree::ItemTree;
use crate::nameres::{DefMap, ScopeOrigin};
use crate::{
    AliasParamId, AliasParamLoc, BlockId, BlockLoc, BranchId, BranchLoc, DefWithBodyId,
    DisciplineAttrId, DisciplineAttrLoc, DisciplineId, DisciplineLoc, FunctionArgId,
    FunctionArgLoc, FunctionId, FunctionLoc, ModuleId, ModuleLoc, NatureAttrId, NatureAttrLoc,
    NatureId, NatureLoc, NodeId, NodeLoc, ParamId, ParamLoc, VarId, VarLoc,
};

#[salsa::query_group(InternDatabase)]
pub trait InternDB: BaseDB {
    #[salsa::interned]
    fn intern_nature(&self, loc: NatureLoc) -> NatureId;
    #[salsa::interned]
    fn intern_nature_attr(&self, loc: NatureAttrLoc) -> NatureAttrId;
    #[salsa::interned]
    fn intern_discipline(&self, loc: DisciplineLoc) -> DisciplineId;
    #[salsa::interned]
    fn intern_discipline_attr(&self, loc: DisciplineAttrLoc) -> DisciplineAttrId;
    #[salsa::interned]
    fn intern_module(&self, loc: ModuleLoc) -> ModuleId;
    #[salsa::interned]
    fn intern_node(&self, loc: NodeLoc) -> NodeId;
    #[salsa::interned]
    fn intern_branch(&self, loc: BranchLoc) -> BranchId;
    #[salsa::interned]
    fn intern_var(&self, loc: VarLoc) -> VarId;
    #[salsa::interned]
    fn intern_param(&self, loc: ParamLoc) -> ParamId;
    #[salsa::interned]
    fn intern_aliasparam(&self, loc: AliasParamLoc) -> AliasParamId;
    #[salsa::interned]
    fn intern_function(&self, loc: FunctionLoc) -> FunctionId;
    #[salsa::interned]
    fn intern_function_arg(&self, loc: FunctionArgLoc) -> FunctionArgId;
    #[salsa::interned]
    fn intern_block(&self, loc: BlockLoc) -> BlockId;
}

#[salsa::query_group(HirDefDatabase)]
pub trait HirDefDB: InternDB + Upcast<dyn BaseDB> {
    #[salsa::invoke(ItemTree::query)]
    fn item_tree(&self, root_file: FileId) -> Arc<ItemTree>;

    #[salsa::invoke(DefMap::root_query)]
    fn root_def_map(&self, root_file: FileId) -> Arc<DefMap>;
    #[salsa::invoke(DefMap::block_query)]
    fn block_def_map(&self, block: BlockId) -> Option<Arc<DefMap>>;
    #[salsa::invoke(DefMap::function_query)]
    fn function_def_map(&self, fun: FunctionId) -> Arc<DefMap>;

    #[salsa::invoke(Body::body_with_srcmap_query)]
    fn body_with_srcmap(&self, id: DefWithBodyId) -> (Arc<Body>, Arc<BodySourceMap>);
    #[salsa::invoke(Body::param_body_with_srcmap_query)]
    fn param_body_with_srcmap(&self, id: ParamId) -> (Arc<Body>, Arc<BodySourceMap>, ParamExprs);
    #[salsa::transparent]
    fn body(&self, id: DefWithBodyId) -> Arc<Body>;
    #[salsa::transparent]
    fn body_srcmap(&self, def: DefWithBodyId) -> Arc<BodySourceMap>;
    #[salsa::transparent]
    fn param_exprs(&self, id: ParamId) -> ParamExprs;

    #[salsa::invoke(NatureData::query)]
    fn nature_data(&self, nature: NatureId) -> Arc<NatureData>;
    #[salsa::invoke(DisciplineData::query)]
    fn discipline_data(&self, discipline: DisciplineId) -> Arc<DisciplineData>;
    #[salsa::invoke(ModuleData::query)]
    fn module_data(&self, module: ModuleId) -> Arc<ModuleData>;
    #[salsa::invoke(NodeData::query)]
    fn node_data(&self, node: NodeId) -> Arc<NodeData>;
    #[salsa::invoke(BranchData::query)]
    fn branch_data(&self, node: BranchId) -> Arc<BranchData>;
    #[salsa::invoke(VarData::query)]
    fn var_data(&self, var: VarId) -> Arc<VarData>;
    #[salsa::invoke(ParamData::query)]
    fn param_data(&self, param: ParamId) -> Arc<ParamData>;
    #[salsa::invoke(AliasParamData::query)]
    fn aliasparam_data(&self, param: AliasParamId) -> Arc<AliasParamData>;
    #[salsa::invoke(FunctionData::query)]
    fn function_data(&self, node: FunctionId) -> Arc<FunctionData>;

    #[salsa::transparent]
    fn find_module(&self, root_file: FileId) -> ModuleId;
}

fn body(db: &dyn HirDefDB, def: DefWithBodyId) -> Arc<Body> {
    db.body_with_srcmap(def).0
}

fn body_srcmap(db: &dyn HirDefDB, def: DefWithBodyId) -> Arc<BodySourceMap> {
    db.body_with_srcmap(def).1
}

fn param_exprs(db: &dyn HirDefDB, param: ParamId) -> ParamExprs {
    db.param_body_with_srcmap(param).2
}

fn find_module(db: &dyn HirDefDB, root_file: FileId) -> ModuleId {
    let def_map = db.root_def_map(root_file);
    let root_scope = def_map.entry_scope();
    def_map[root_scope]
        .children
        .values()
        .find_map(|scope| {
            if let ScopeOrigin::Module(module) = def_map[*scope].origin {
                Some(module)
            } else {
                None
            }
        })
        .expect("No Module found")
}
