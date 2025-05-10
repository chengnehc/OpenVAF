//! HIR provides a high-level object oriented access to Verilog-A code.
//!
//! HIR is the public API of all the compiler logic above syntax trees.
//! It is written in "OO" style. Each type is **self-contained** (as in,
//! it knows its parents and full context). It should be "clean code".
//!
//! `hir_*` crates are the implementation of the compiler logic. They are
//! written in "ECS" style, with relatively little abstractions. Many types
//! are not self-contained, and explicitly use local indexes, arenas, etc.

use std::sync::Arc;
use stdx::impl_debug;

use basedb::{BaseDB, FileId};
use hir_def::db::HirDefDB;
use hir_def::nameres::{DefMap, LocalScopeId, ScopeItemDef};
use hir_def::{
    AliasParamId, BlockId, BranchId, DefWithBodyId, DisciplineId, FunctionId, LocalFunctionArgId,
    Lookup, ModuleId, ModuleLoc, NatureAttrId, NatureId, NodeId, ParamId, VarId,
};
use hir_ty::db::HirTyDB as HirDB;
use hir_ty::inference;
use salsa::InternKey;
use smol_str::SmolStr;
use syntax::ast;

pub use basedb::diagnostics::DiagnosticSink;
pub use hir_def::body::{Case, CaseCond, ConstraintValue, Literal, ParamConstraint};
pub use hir_def::nameres::PathResolveError;
pub use hir_def::{BuiltIn, ParamSysFun, Path, Type};
pub use hir_ty::builtin;
pub use rec_declarations::RecDeclarations;
pub use syntax::name::Name;

pub mod diagnostics;
pub mod signatures {
    pub use hir_ty::builtin::{
        ABSDELAY_MAX, ABS_INT, ABS_REAL, DDX_POT, IDTMOD_IC, IDTMOD_IC_MODULUS,
        IDTMOD_IC_MODULUS_OFFSET, IDTMOD_IC_MODULUS_OFFSET_NATURE, IDTMOD_IC_MODULUS_OFFSET_TOL,
        IDTMOD_NO_IC, IDT_IC, IDT_IC_ASSERT, IDT_IC_ASSERT_NATURE, IDT_IC_ASSERT_TOL, IDT_NO_IC,
        LIMIT_BUILTIN_FUNCTION, MAX_INT, MAX_REAL, NATURE_ACCESS_BRANCH, NATURE_ACCESS_NODES,
        NATURE_ACCESS_NODE_GND, NATURE_ACCESS_PORT_FLOW, SIMPARAM_DEFAULT, SIMPARAM_NO_DEFAULT,
    };
    pub use hir_ty::types::{BOOL_EQ, INT_EQ, INT_OP, REAL_EQ, REAL_OP, STR_EQ};
}

mod attributes;
mod body;
mod db;
mod rec_declarations;

pub use attributes::AstCache;
pub use body::{
    AssignmentLhs, Body, BodyRef, ContributeKind, Expr, ExprId, Ref, ResolvedFun, Stmt, StmtId,
};
pub use db::CompilationDB;

/// A compilation unit is represented by a root file (entry file).
///
/// As a result of '`include' statements,
/// - a compilation unit may contain multiple physical files
/// - a physical file may be a part of multiple compilation units
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CompilationUnit {
    root_file: FileId,
}

impl CompilationUnit {
    pub fn root_file(self) -> FileId {
        self.root_file
    }

    pub fn name(self, db: &CompilationDB) -> String {
        db.file_path(self.root_file).name().unwrap_or_else(|| String::from("~.va"))
    }

    pub fn preprocess(&self, db: &CompilationDB) -> syntax::Preprocess {
        db.preprocess(self.root_file)
    }

    /// Create an ast cache to extract information of attributes, used by simulator backend
    pub fn ast_cache(&self, db: &CompilationDB) -> attributes::AstCache {
        attributes::AstCache::new(db, self.root_file)
    }

    pub fn modules(self, db: &CompilationDB) -> Vec<Module> {
        let root_def_map = db.root_def_map(self.root_file);
        let entry = root_def_map.entry_scope();
        root_def_map[entry]
            .declarations
            .iter()
            .filter_map(|(_, def)| {
                if let ScopeItemDef::ModuleId(id) = *def {
                    Some(Module { id })
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn collect_diagnostics(self, db: &CompilationDB, sink: &mut impl DiagnosticSink) {
        diagnostics::collect(db, self.root_file, sink)
    }

    pub fn test_diagnostics(&self, db: &CompilationDB) -> String {
        use basedb::diagnostics::sink::Buffer;
        use basedb::diagnostics::ConsoleSink;

        let mut buf = Buffer::no_color();
        {
            let mut sink = ConsoleSink::buffer(db, &mut buf);
            sink.annonymize_paths();
            self.collect_diagnostics(db, &mut sink);
        }
        let data = buf.into_inner();

        String::from_utf8(data).unwrap()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Nature {
    id: NatureId,
}
impl Nature {
    pub fn name(self, db: &CompilationDB) -> String {
        db.nature_data(self.id).name.to_string()
    }
    pub fn units(self, db: &CompilationDB) -> String {
        db.nature_data(self.id).units.clone().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NatureAttr {
    id: NatureAttrId,
}
impl NatureAttr {
    pub fn value(&self, db: &CompilationDB) -> Body {
        Body::new(self.id.into(), db)
    }
    pub fn name(self, db: &CompilationDB) -> String {
        let loc = self.id.lookup(db);
        db.nature_data(loc.nature).attrs[loc.id].name.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Discipline {
    id: DisciplineId,
}
impl Discipline {
    pub fn name(self, db: &CompilationDB) -> String {
        db.discipline_data(self.id).name.to_string()
    }
    pub fn potential(&self, db: &CompilationDB) -> Option<Nature> {
        db.discipline_info(self.id).potential.map(|id| Nature { id })
    }
    pub fn flow(&self, db: &CompilationDB) -> Option<Nature> {
        db.discipline_info(self.id).flow.map(|id| Nature { id })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Module {
    id: ModuleId,
}
impl_debug! {
    match Module { Module { id } => "{id:?}"; }
}
impl Module {
    pub fn name(self, db: &CompilationDB) -> String {
        db.module_data(self.id).name.to_string()
    }
    pub fn ports(self, db: &CompilationDB) -> Vec<Node> {
        db.module_data(self.id).ports.iter().map(|&id| Node { id }).collect()
    }
    pub fn internal_nodes(self, db: &CompilationDB) -> Vec<Node> {
        db.module_data(self.id).internal_nodes.iter().map(|&id| Node { id }).collect()
    }
    pub fn uuid(self, _db: &CompilationDB) -> u32 {
        self.id.as_intern_id().as_u32()
    }
    /* JW: not used
        /// list of all child scopes.
        pub fn child_scopes(self, db: &CompilationDB) -> Vec<Scope> {
            Scope::Module(self).children(db)
        }
        /// list of all declarations.
        pub fn declarations(self, db: &CompilationDB) -> Vec<(Name, ScopeDef)> {
            Scope::Module(self).declarations(db)
        }
    */
    pub fn rec_declarations(self, db: &CompilationDB) -> RecDeclarations<'_> {
        RecDeclarations::new(Scope::Module(self), db)
    }
    pub fn analog_initial_body(&self, db: &CompilationDB) -> Body {
        Body::new(DefWithBodyId::ModuleId { initial: true, id: self.id }, db)
    }
    pub fn analog_body(&self, db: &CompilationDB) -> Body {
        Body::new(DefWithBodyId::ModuleId { initial: false, id: self.id }, db)
    }
    // JW: for VerilogAE
    pub fn lookup_var(
        &self,
        db: &CompilationDB,
        path: &Path,
    ) -> Result<Variable, PathResolveError> {
        let scope = self.lookup(db).scope;
        scope.resolve_item_path(db, path).map(|id| Variable { id })
    }
    fn lookup(self, db: &CompilationDB) -> ModuleLoc {
        self.id.lookup(db)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Node {
    id: NodeId,
}
impl_debug! {
    match Node { Node { id } => "{id:?}"; }
}
impl Node {
    #[inline]
    pub fn name(self, db: &CompilationDB) -> SmolStr {
        db.node_data(self.id).name.clone().into()
    }
    #[inline]
    pub fn discipline(self, db: &CompilationDB) -> Discipline {
        let id = db.node_discipline(self.id).unwrap();
        Discipline { id }
    }
    #[inline]
    pub fn is_input(self, db: &CompilationDB) -> bool {
        db.node_data(self.id).is_input
    }
    #[inline]
    pub fn is_output(self, db: &CompilationDB) -> bool {
        db.node_data(self.id).is_output
    }
    #[inline]
    pub fn is_port(self, db: &CompilationDB) -> bool {
        db.node_data(self.id).is_port()
    }
    #[inline]
    pub fn is_gnd(self, db: &CompilationDB) -> bool {
        db.node_data(self.id).is_gnd
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Branch {
    id: BranchId,
}
impl_debug! {
    match Branch { Branch { id } => "{id:?}"; }
}
impl Branch {
    pub fn name(self, db: &CompilationDB) -> String {
        db.branch_data(self.id).name.to_string()
    }
    pub fn discipline(self, db: &CompilationDB) -> Discipline {
        let id = db.branch_info(self.id).unwrap().discipline;
        Discipline { id }
    }
    pub fn kind(self, db: &CompilationDB) -> BranchKind {
        match db.branch_info(self.id).unwrap().kind {
            hir_ty::lower::BranchKind::PortFlow(node) => BranchKind::PortFlow(Node { id: node }),
            hir_ty::lower::BranchKind::NodeGnd(node) => BranchKind::NodeGnd(Node { id: node }),
            hir_ty::lower::BranchKind::Nodes(hi, lo) => {
                BranchKind::Nodes(Node { id: hi }, Node { id: lo })
            }
        }
    }
    pub fn get_attr(&self, db: &CompilationDB, ast: &AstCache, name: &str) -> Option<ast::Attr> {
        ast.resolve_attr(name, self.id.lookup(db).ast_id(db))
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum BranchKind {
    PortFlow(Node),
    NodeGnd(Node),
    Nodes(Node, Node),
}
impl BranchKind {
    pub fn unwrap_hi_node(self) -> Node {
        match self {
            BranchKind::NodeGnd(hi) | BranchKind::Nodes(hi, _) => hi,
            BranchKind::PortFlow(_) => unreachable!(),
        }
    }
    pub fn lo_node(self) -> Option<Node> {
        match self {
            BranchKind::Nodes(_, lo) => Some(lo),
            _ => None,
        }
    }
}

/// Namely `branch_lvalue` as specified in [LRM 5.6.1]
#[derive(Debug, Clone, PartialEq, Eq, Copy, Hash)]
pub enum BranchWrite {
    Named(Branch),
    Unnamed { hi: Node, lo: Option<Node> },
}
impl BranchWrite {
    pub fn node_pair(self, db: &CompilationDB) -> (Node, Option<Node>) {
        match self {
            BranchWrite::Named(branch) => match branch.kind(db) {
                BranchKind::Nodes(hi, lo) => (hi, Some(lo)),
                BranchKind::NodeGnd(hi) => (hi, None),
                BranchKind::PortFlow(_) => unreachable!(),
            },
            BranchWrite::Unnamed { hi, lo } => (hi, lo),
        }
    }
}
impl From<inference::BranchWrite> for BranchWrite {
    #[inline]
    fn from(inner: inference::BranchWrite) -> Self {
        match inner {
            inference::BranchWrite::Named(branch) => BranchWrite::Named(Branch { id: branch }),
            inference::BranchWrite::Unnamed { hi, lo } => {
                BranchWrite::Unnamed { hi: Node { id: hi }, lo: lo.map(|id| Node { id }) }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Block {
    id: BlockId,
}
impl_debug! {
    match Block { Block{ id } => "{id:?}"; }
}
impl Block {
    pub fn name(self, db: &CompilationDB) -> String {
        self.id.lookup(db).name(db).to_string()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Variable {
    id: VarId,
}
impl_debug! {
    match Variable { Variable { id } => "{id:?}"; }
}
impl Variable {
    pub fn name(self, db: &CompilationDB) -> SmolStr {
        db.var_data(self.id).name.clone().into()
    }
    pub fn ty(self, db: &CompilationDB) -> Type {
        db.var_data(self.id).ty.clone()
    }
    pub fn init(self, db: &CompilationDB) -> Body {
        Body::new(self.id.into(), db)
    }
    pub fn get_attr(&self, db: &CompilationDB, ast: &AstCache, name: &str) -> Option<ast::Attr> {
        ast.resolve_attr(name, self.id.lookup(db).ast_id(db))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Parameter {
    id: ParamId,
}
impl Parameter {
    pub fn name(self, db: &CompilationDB) -> String {
        db.param_data(self.id).name.to_string()
    }
    pub fn ty(self, db: &CompilationDB) -> Type {
        db.param_ty(self.id)
    }
    pub fn default(self, db: &CompilationDB) -> ExprId {
        db.param_exprs(self.id).default
    }
    pub fn constraints(self, db: &CompilationDB) -> Arc<[ParamConstraint]> {
        db.param_exprs(self.id).constraints
    }
    pub fn init(self, db: &CompilationDB) -> Body {
        Body::new(self.id.into(), db)
    }
    pub fn get_attr(&self, db: &CompilationDB, ast: &AstCache, name: &str) -> Option<ast::Attr> {
        ast.resolve_attr(name, self.id.lookup(db).ast_id(db))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AliasParam {
    id: AliasParamId,
}
impl_debug! {
    match AliasParam { AliasParam { id } => "{id:?}"; }
}
impl AliasParam {
    pub fn name(self, db: &CompilationDB) -> String {
        db.aliasparam_data(self.id).name.to_string()
    }
    pub fn resolve(self, db: &CompilationDB) -> Option<ResolvedAliasParam> {
        db.resolve_alias(self.id).and_then(|alias| match alias {
            hir_ty::db::Alias::Cycle => None,
            hir_ty::db::Alias::Param(id) => Some(ResolvedAliasParam::Parameter(Parameter { id })),
            hir_ty::db::Alias::ParamSysFun(param) => Some(ResolvedAliasParam::Sysfun(param)),
        })
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub enum ResolvedAliasParam {
    Parameter(Parameter),
    Sysfun(ParamSysFun),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Function {
    id: FunctionId,
}
impl_debug! {
    match Function { Function { id } => "{id:?}"; }
}
impl Function {
    pub fn name(self, db: &CompilationDB) -> String {
        db.function_data(self.id).name.to_string()
    }
    pub fn return_ty(self, db: &CompilationDB) -> Type {
        db.function_data(self.id).return_ty.clone()
    }
    pub fn args(self, db: &CompilationDB) -> impl Iterator<Item = FunctionArg> + Clone {
        let num = db.function_data(self.id).args.len();
        (0..num).map(move |i| FunctionArg { fun_id: self.id, arg_id: i.into() })
    }
    pub fn arg(self, idx: usize, db: &CompilationDB) -> FunctionArg {
        // AB: debug assertion should be > because length>index
        debug_assert!(db.function_data(self.id).args.len() > idx);
        FunctionArg { fun_id: self.id, arg_id: idx.into() }
    }
    pub fn body(&self, db: &CompilationDB) -> Body {
        Body::new(self.id.into(), db)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionArg {
    fun_id: FunctionId,
    arg_id: LocalFunctionArgId,
}
impl FunctionArg {
    pub fn function(self) -> Function {
        Function { id: self.fun_id }
    }
    pub fn name(self, db: &CompilationDB) -> String {
        db.function_data(self.fun_id).args[self.arg_id].name.to_string()
    }
    pub fn ty(self, db: &CompilationDB) -> Type {
        db.function_data(self.fun_id).args[self.arg_id].ty.clone()
    }
    pub fn is_input(self, db: &CompilationDB) -> bool {
        db.function_data(self.fun_id).args[self.arg_id].is_input
    }
    pub fn is_output(self, db: &CompilationDB) -> bool {
        db.function_data(self.fun_id).args[self.arg_id].is_output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scope {
    Module(Module),
    Block(Block),
    Function(Function),
}
impl Scope {
    fn def_map_and_scope(self, db: &CompilationDB) -> (LocalScopeId, Arc<DefMap>) {
        match self {
            Scope::Module(module) => {
                let loc = module.lookup(db);
                (loc.scope.local_id, loc.def_map(db))
            }
            Scope::Block(block) => {
                let def_map =
                    db.block_def_map(block.id).expect("block should be named to have scope");
                (def_map.entry_scope(), def_map)
            }
            Scope::Function(func) => {
                let def_map = db.function_def_map(func.id);
                (def_map.entry_scope(), def_map)
            }
        }
    }

    /*
        /// Iterates over all child modules.
        pub fn children(self, db: &CompilationDB) -> Vec<Scope> {
            let (scope, def_map) = self.def_map_and_scope(db);
            def_map[scope]
                .children
                .values()
                .map(|&scope| match def_map[scope].origin {
                    hir_def::nameres::ScopeOrigin::Root => {
                        unreachable!("Root scope can not be a child scope")
                    }
                    hir_def::nameres::ScopeOrigin::Module(id) => Scope::Module(Module { id }),
                    hir_def::nameres::ScopeOrigin::Block(id) => Scope::Block(Block { id }),
                    hir_def::nameres::ScopeOrigin::Function(id) => Scope::Function(Function { id }),
                })
                .collect()
        }

        // Iterates over all child declarations.
        pub fn declarations(self, db: &CompilationDB) -> Vec<(Name, ScopeDef)> {
            let (scope, def_map) = self.def_map_and_scope(db);
            def_map[scope]
                .declarations
                .iter()
                .filter_map(|(name, &def)| {
                    let res = match def {
                        ScopeDefItem::ModuleId(id) => ScopeDef::ModuleInstance(Module { id }),
                        ScopeDefItem::BlockId(id) => ScopeDef::Block(Block { id }),
                        ScopeDefItem::NodeId(id) => ScopeDef::Node(Node { id }),
                        ScopeDefItem::VarId(id) => ScopeDef::Variable(Variable { id }),
                        ScopeDefItem::ParamId(id) => ScopeDef::Parameter(Parameter { id }),
                        ScopeDefItem::AliasParamId(id) => {
                            ScopeDef::AliasParameter(AliasParameter { id })
                        }
                        ScopeDefItem::BranchId(id) => ScopeDef::Branch(Branch { id }),
                        ScopeDefItem::FunctionId(id) => ScopeDef::Function(Function { id }),
                        // implementation details
                        ScopeDefItem::BuiltIn(_)
                        | ScopeDefItem::NatureId(_)
                        | ScopeDefItem::NatureAccess(_)
                        | ScopeDefItem::DisciplineId(_)
                        | ScopeDefItem::ParamSysFun(_)
                        | ScopeDefItem::FunctionReturn(_)
                        | ScopeDefItem::FunctionArgId(_)
                        | ScopeDefItem::NatureAttrId(_) => return None,
                    };
                    Some((name.to_owned(), res))
                })
                .collect()
        }
    */
}

#[non_exhaustive]
#[derive(Debug)]
pub enum ScopeDef {
    Module(Module),
    Node(Node),
    Branch(Branch),
    Variable(Variable),
    Parameter(Parameter),
    AliasParam(AliasParam),
    Block(Block),
    Function(Function),
}
