use std::sync::Arc;
use stdx::Upcast;

use hir_def::{
    db::HirDefDB,
    nameres::{ItemWithBodyId, PathResolveError, ScopeItem},
    AliasParamId, BranchId, DisciplineId, Lookup, NatureAttrId, NatureId, NodeId, ParamId,
    ParamSysFun, Type,
};

use crate::inference::Inference;
use crate::lower::{BranchTy, DisciplineTy, NatureTy, PathError};

#[salsa::query_group(HirTyDatabase)]
pub trait HirTyDB: HirDefDB + Upcast<dyn HirDefDB> {
    #[salsa::invoke(NatureTy::nature_info_query)]
    #[salsa::cycle(NatureTy::nature_info_recover)]
    fn nature_info(&self, nature: NatureId) -> Result<Arc<NatureTy>, PathError>;

    #[salsa::invoke(DisciplineTy::discipline_info_query)]
    fn discipline_info(&self, disc: DisciplineId) -> Result<Arc<DisciplineTy>, PathError>;

    #[salsa::invoke(BranchTy::branch_info_query)]
    fn branch_info(&self, branch: BranchId) -> Option<Arc<BranchTy>>;

    #[salsa::invoke(Inference::infere_body_query)]
    fn inference_result(&self, id: ItemWithBodyId) -> Arc<Inference>;

    #[salsa::cycle(nature_attr_ty_recover)]
    fn nature_attr_ty(&self, nature_attr: NatureAttrId) -> Option<Type>;

    #[salsa::transparent]
    fn param_ty(&self, param: ParamId) -> Type;

    fn node_discipline(&self, node: NodeId) -> Option<DisciplineId>;

    #[salsa::cycle(resolve_alias_recover)]
    fn resolve_alias(&self, alias: AliasParamId) -> Result<Alias, PathResolveError>;

    #[salsa::input]
    fn known_limit_functions(&self) -> Option<Arc<[LimitSignature]>>;
}

#[derive(Debug, PartialEq, Eq, Clone, Hash)]
pub struct LimitSignature {
    pub name: String,
    pub num_args: u32,
}

fn nature_attr_ty(db: &dyn HirTyDB, id: NatureAttrId) -> Option<Type> {
    let id = id.into();
    let body = db.body(id);
    let stmt_expr = body.entry_stmts()[0];
    let expr = body[stmt_expr].unwrap_expr();
    db.inference_result(id).expr_types.get(expr).and_then(|ty| ty.to_value())
}

// TODO proper cycle recovery
#[allow(clippy::trivially_copy_pass_by_ref)]
fn nature_attr_ty_recover(
    _db: &dyn HirTyDB,
    _cycel: &salsa::Cycle,
    _id: &NatureAttrId,
) -> Option<Type> {
    None
}

fn param_ty(db: &dyn HirTyDB, param: ParamId) -> Type {
    db.param_data(param).ty.clone().unwrap_or_else(|| {
        let default_expr = db.param_exprs(param).default;
        let default_ty = &db.inference_result(param.into()).expr_types[default_expr];
        default_ty.to_value().unwrap_or(Type::Err)
    })
}

fn node_discipline(db: &dyn HirTyDB, id: NodeId) -> Option<DisciplineId> {
    let node = db.node_data(id);
    node.discipline.as_ref().and_then(|discipline| {
        let db = db.upcast();
        let def_map = id.lookup(db).module.lookup(db).def_map(db);
        let root_scope = def_map.root_scope();
        def_map.resolve_item_name(root_scope, discipline).ok()
    })
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum Alias {
    Cycle,
    Param(ParamId),
    ParamSysFun(ParamSysFun),
}

fn resolve_alias(db: &dyn HirTyDB, id: AliasParamId) -> Result<Alias, PathResolveError> {
    let alias = db.aliasparam_data(id);
    let scope = id.lookup(db.upcast()).scope;
    let name = &alias.param_ref;

    match scope.resolve_name(db.upcast(), &alias.param_ref)? {
        ScopeItem::ParamId(param) => Ok(Alias::Param(param)),
        ScopeItem::ParamSysFun(fun) => Ok(Alias::ParamSysFun(fun)),
        ScopeItem::AliasParamId(alias) => db.resolve_alias(alias),
        found => Err(PathResolveError::ExpectedItemKind {
            name: name.clone(),
            expected: "parameter or system parameter",
            found: found.into(),
        }),
    }
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn resolve_alias_recover(
    _db: &dyn HirTyDB,
    _cycel: &salsa::Cycle,
    _id: &AliasParamId,
) -> Result<Alias, PathResolveError> {
    Ok(Alias::Cycle)
}
