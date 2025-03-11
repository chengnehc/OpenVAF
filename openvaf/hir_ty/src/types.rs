use std::borrow::Cow;
use std::ops::Deref;
use stdx::{impl_display, impl_idx_from, pretty};

use hir_def::{
    BranchId, DisciplineId, FunctionId, LocalFunctionArgId, NatureAttrId, NatureId, NodeId,
    ParamId, Type, VarId,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Nature(NatureId),
    Node(NodeId),
    Branch(BranchId),
    PortFlow(NodeId),
    UserFunction(FunctionId),

    InfLiteral,
    Literal(Type),
    Val(Type),
    NatureAttr(Type, NatureAttrId),
    Var(Type, VarId),
    Param(Type, ParamId),
    FunctionVar { ty: Type, fun: FunctionId, arg: Option<LocalFunctionArgId> },

    Discipline(DisciplineId),
    Scope,
    BuiltInFunction,
}

// TODO coerce tys for nicer errors? Would that even be an improvement?

impl_display! {
    match Ty{
        Ty::Nature(_) => "nature reference";
        Ty::NatureAttr(ty,_) => "{} nature attribute reference", ty;
        Ty::Discipline(_) => "discipline reference";
        Ty::Literal(ty) => "{} literal", ty;
        Ty::InfLiteral => "numeric literal";
        Ty::Val(ty) => "{} value",ty;
        Ty::Node(_) => "net reference";
        Ty::Branch(_) => "branch reference";
        Ty::PortFlow(_) => "port-flow reference";
        Ty::Var(ty,_) => "{} variable reference", ty;
        Ty::Param(ty,_) => "{} parameter reference", ty;
        Ty::UserFunction(_) => "(user-defined) function";
        Ty::FunctionVar{ty,..} => "{} variable reference", ty;
        Ty::BuiltInFunction => "(builtin) function";
        Ty::Scope => "scope";
    }
}

impl Ty {
    pub fn unwrap_node(&self) -> NodeId {
        let Ty::Node(id) = *self else { unreachable!("expected node, found {:?}", self) };
        id
    }

    pub fn unwrap_branch(&self) -> BranchId {
        let Ty::Branch(id) = *self else { unreachable!("expected branch, found {:?}", self) };
        id
    }

    pub fn unwrap_port_flow(&self) -> NodeId {
        let Ty::PortFlow(id) = *self else { unreachable!("expected port, found {:?}", self) };
        id
    }

    pub fn unwrap_param(&self) -> ParamId {
        let Ty::Param(_, id) = *self else { unreachable!("expected parameter, found {:?}", self) };
        id
    }
    pub fn unwrap_func(&self) -> FunctionId {
        let Ty::UserFunction(func) = *self else { unreachable!("called unwrap_func on {self:?}") };
        func
    }

    pub fn to_value(&self) -> Option<Type> {
        match self {
            Ty::Val(ty)
            | Ty::Var(ty, _)
            | Ty::NatureAttr(ty, _)
            | Ty::Param(ty, _)
            | Ty::Literal(ty)
            | Ty::FunctionVar { ty, .. } => Some(ty.clone()),
            Ty::InfLiteral => Some(Type::Real),
            _ => None,
        }
    }

    pub fn satisfies_semantic(&self, requirement: &TyRequirement) -> bool {
        self.satisfies(requirement, TyEquivalence::Semantic)
    }

    pub fn satisfies_with_conversion(&self, requirement: &TyRequirement) -> bool {
        self.satisfies(requirement, TyEquivalence::Conversion)
    }

    pub fn satisfies_exact(&self, requirement: &TyRequirement) -> bool {
        self.satisfies(requirement, TyEquivalence::Exact)
    }

    fn satisfies(&self, requirement: &TyRequirement, equiv: TyEquivalence) -> bool {
        match (self, requirement) {
            (Ty::Nature(_), TyRequirement::Nature)
            | (Ty::Node(_), TyRequirement::Node)
            | (Ty::Branch(_), TyRequirement::Branch)
            | (Ty::PortFlow(_), TyRequirement::PortFlow)
            | (Ty::UserFunction(_), TyRequirement::Function)
            | (Ty::InfLiteral, TyRequirement::Val(Type::Real)) // special case of `inf`
            | (
                Ty::Val(Type::EmptyArray | Type::Array { len: 0, .. }),
                TyRequirement::ArrayAnyLength { .. }
                | TyRequirement::Val(Type::Array { len: 0, .. }),
            ) // special case of empty array
            // predicates
            | (Ty::Param(_, _), TyRequirement::AnyParam)
            | (
                Ty::Val(_)
                | Ty::Var(_, _)
                | Ty::NatureAttr(_, _)
                | Ty::Param(_, _)
                | Ty::InfLiteral
                | Ty::Literal(_)
                | Ty::FunctionVar { .. },
                TyRequirement::AnyVal,
            ) => true,

            (
                Ty::Val(ty1)
                | Ty::Literal(ty1)
                | Ty::Var(ty1, _)
                | Ty::NatureAttr(ty1, _)
                | Ty::Param(ty1, _)
                | Ty::FunctionVar { ty: ty1, .. },
                TyRequirement::Val(ty2),
            )
            | (Ty::Literal(ty1), TyRequirement::Literal(ty2)) => equiv.compare_ty(ty1, ty2),

            (
                Ty::Val(ty)
                | Ty::Var(ty, _)
                | Ty::NatureAttr(ty, _)
                | Ty::Param(ty, _)
                | Ty::Literal(ty)
                | Ty::FunctionVar { ty, .. },
                TyRequirement::Condition,
            ) => ty.is_assignable_to(&Type::Bool),

            // TODO merge these match arms when there are box/deref patterns (not any time soon)
            (
                Ty::Val(Type::Array { ty: ref ty1, .. }),
                TyRequirement::ArrayAnyLength { ty: ty2 },
            ) => equiv.compare_ty(ty1, ty2),

            // No conversion for explicit references
            (
                Ty::Var(ty1, _) | Ty::NatureAttr(ty1, _) | Ty::FunctionVar { ty: ty1, .. },
                TyRequirement::Var(ty2),
            )
            | (Ty::Param(ty1, _), TyRequirement::Param(ty2)) => ty1 == ty2,

            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TyRequirement {
    // explicit requirement
    Nature,
    Node,
    Branch,
    PortFlow,
    Function,
    // implicit requirement
    Literal(Type),
    Val(Type),
    Var(Type),
    Param(Type),
    ArrayAnyLength { ty: Type },
    // predicates
    AnyVal,    // any type of `Val`
    AnyParam,  // any type of `Param`
    Condition, // must be bool type
}

impl TyRequirement {
    pub fn cast(&self, src: &Type) -> Option<Type> {
        match self {
            TyRequirement::Val(ty) if src != ty => Some(ty.to_owned()),
            TyRequirement::Condition if src != &Type::Bool => Some(Type::Bool),
            _ => None,
        }
    }
}

impl_display! {
    match TyRequirement{
        TyRequirement::Nature => "nature reference";
        TyRequirement::Node => "net reference";
        TyRequirement::Branch => "branch reference";
        TyRequirement::PortFlow => "port-flow reference";
        TyRequirement::Function => "function";
        TyRequirement::Literal(ty) => "{} literal", ty;
        TyRequirement::Val(ty) => "{} value", ty;
        TyRequirement::Var(ty) => "{} variable reference", ty;
        TyRequirement::Param(ty) => "{} parameter reference", ty;
        TyRequirement::ArrayAnyLength{ty} => "array ({})", ty;
        TyRequirement::AnyVal => "value";
        TyRequirement::AnyParam => "parameter reference";
        TyRequirement::Condition => "{} value", Type::Bool;
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
enum TyEquivalence {
    Conversion,
    Semantic,
    Exact,
}

impl TyEquivalence {
    fn compare_ty(self, ty1: &Type, ty2: &Type) -> bool {
        match self {
            TyEquivalence::Conversion => ty1.is_convertible_to(ty2),
            TyEquivalence::Semantic => ty1.is_semantically_eq_to(ty2),
            TyEquivalence::Exact => ty1 == ty2,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Signature(pub u32);
impl_idx_from!(Signature(u32));

pub const BOOL_EQ: Signature = Signature(0);
pub const INT_EQ: Signature = Signature(1);
pub const REAL_EQ: Signature = Signature(2);
pub const STR_EQ: Signature = Signature(3);

pub const INT_OP: Signature = Signature(0);
pub const REAL_OP: Signature = Signature(1);

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct SignatureData {
    pub args: Cow<'static, [TyRequirement]>,
    pub return_ty: Type,
}
impl_display! {
    match SignatureData{
        SignatureData{ args, return_ty } => "({}) -> {}", pretty::List::new(args.deref()).with_final_separator(", "), return_ty;
    }
}

impl SignatureData {
    pub const INT_BIN_OP: SignatureData = SignatureData {
        args: Cow::Borrowed(&[
            TyRequirement::Val(Type::Integer),
            TyRequirement::Val(Type::Integer),
        ]),
        return_ty: Type::Integer,
    };
    pub const REAL_BIN_OP: SignatureData = SignatureData {
        args: Cow::Borrowed(&[TyRequirement::Val(Type::Real), TyRequirement::Val(Type::Real)]),
        return_ty: Type::Real,
    };
    pub const BOOL_BIN_OP: SignatureData = SignatureData {
        args: Cow::Borrowed(&[TyRequirement::Val(Type::Bool), TyRequirement::Val(Type::Bool)]),
        return_ty: Type::Bool,
    };
    pub const CONDITIONAL_BIN_OP: SignatureData = SignatureData {
        args: Cow::Borrowed(&[TyRequirement::Condition, TyRequirement::Condition]),
        return_ty: Type::Bool,
    };
    pub const NUMERIC_BIN_OP: &'static [SignatureData] =
        &[SignatureData::INT_BIN_OP, SignatureData::REAL_BIN_OP];
    pub const SELECT_OP: &'static [SignatureData] =
        &[SignatureData::BOOL_BIN_OP, SignatureData::REAL_BIN_OP, SignatureData::INT_BIN_OP];

    pub const REAL_COMPARISON: SignatureData = SignatureData {
        args: Cow::Borrowed(&[TyRequirement::Val(Type::Real), TyRequirement::Val(Type::Real)]),
        return_ty: Type::Bool,
    };
    pub const INT_COMPARISON: SignatureData = SignatureData {
        args: Cow::Borrowed(&[
            TyRequirement::Val(Type::Integer),
            TyRequirement::Val(Type::Integer),
        ]),
        return_ty: Type::Bool,
    };
    pub const STR_COMPARISON: SignatureData = SignatureData {
        args: Cow::Borrowed(&[TyRequirement::Val(Type::String), TyRequirement::Val(Type::String)]),
        return_ty: Type::Bool,
    };
    pub const BOOL_COMPARISON: SignatureData = SignatureData {
        args: Cow::Borrowed(&[TyRequirement::Val(Type::Bool), TyRequirement::Val(Type::Bool)]),
        return_ty: Type::Bool,
    };
    pub const NUMERIC_COMPARISON: &'static [SignatureData] =
        &[SignatureData::INT_COMPARISON, SignatureData::REAL_COMPARISON];
    pub const ANY_COMPARISON: &'static [SignatureData] = &[
        SignatureData::BOOL_COMPARISON,
        SignatureData::INT_COMPARISON,
        SignatureData::REAL_COMPARISON,
        SignatureData::STR_COMPARISON,
    ];
}
