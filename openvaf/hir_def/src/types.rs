//! Type definitions for HIR type inference.
//!
//! Refer to LRM Chapter 3 and Annex C.4

use std::iter::successors;
use stdx::impl_display;
use syntax::ast;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Type {
    Err,
    Void,
    Bool,
    Integer,
    Real,
    String,
    EmptyArray,
    Array { ty: Box<Type>, len: u32 },
}

impl_display! {
    match Type {
        Type::Err => "[missing]";
        Type::Void => "void";
        Type::Bool => "integer";
        Type::Integer => "integer";
        Type::Real => "real";
        Type::String => "string";
        Type::EmptyArray => "_[0:0]";
        Type::Array {ty, len} => "{ty}[0:{len}]";
    }
}

impl Type {
    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::Real | Type::Integer | Type::Bool)
    }

    pub fn is_convertible_to(&self, dst: &Type) -> bool {
        match (self, dst) {
            (Type::Integer | Type::Bool, Type::Real)
            | (Type::Integer, Type::Bool)
            | (Type::Bool, Type::Integer)
            | (Type::Err, _)
            | (_, Type::Err)
            | (Type::EmptyArray, Type::Array { len: 0, .. })
            | (Type::Array { len: 0, .. }, Type::EmptyArray) => true,

            (Type::Array { .. }, Type::Array { .. }) => {
                self.dim() == dst.dim() && self.base_type().is_convertible_to(dst.base_type())
            }

            _ => dst == self,
        }
    }

    pub fn is_semantically_eq_to(&self, other: &Type) -> bool {
        match (self, other) {
            (Type::Integer, Type::Bool)
            | (Type::Bool, Type::Integer)
            | (Type::EmptyArray, Type::Array { len: 0, .. })
            | (Type::Array { len: 0, .. }, Type::EmptyArray) => true,

            (Type::Array { .. }, Type::Array { .. }) => {
                self.dim() == other.dim()
                    && self.base_type().is_semantically_eq_to(other.base_type())
            }

            _ => other == self,
        }
    }

    pub fn union(&self, other: &Type) -> Option<Type> {
        match (self, other) {
            (Type::Real, Type::Integer | Type::Bool) | (Type::Integer | Type::Bool, Type::Real) => {
                Some(Type::Real)
            }
            (Type::Bool, Type::Integer) | (Type::Integer, Type::Bool) => Some(Type::Integer),

            (
                Type::EmptyArray | Type::Array { len: 0, .. },
                Type::EmptyArray | Type::Array { len: 0, .. },
            ) => Some(Type::EmptyArray),

            _ if self == other => Some(self.clone()),
            _ if self.dim() == other.dim() => {
                let base_type1 = self.base_type();
                let base_type2 = self.base_type();
                let ty = base_type1.union(base_type2)?;
                debug_assert_ne!(self.dim(), &[]);
                Some(ty.to_dim(&self.dim()))
            }
            _ => None,
        }
    }

    pub fn base_type(&self) -> &Type {
        let mut curr = self;
        while let Type::Array { ty, .. } = self {
            curr = ty
        }
        curr
    }

    /// Obtain the dimensions of a (nested) array.
    pub fn dim(&self) -> Vec<u32> {
        if let Type::Array { ref ty, len } = *self {
            let mut dims: Vec<_> = successors(Some((ty, len)), |(ty, _)| {
                if let Type::Array { ref ty, len } = ***ty {
                    Some((ty, len))
                } else {
                    None
                }
            })
            .map(|(_, len)| len)
            .collect();
            dims.reverse();
            dims
        } else {
            vec![]
        }
    }

    pub fn is_assignable_to(&self, dst: &Type) -> bool {
        self.is_convertible_to(dst)
            || (self.base_type().is_numeric()
                && dst.base_type().is_numeric()
                && self.dim() == dst.dim())
    }

    /// convert Array to a specified dimension
    #[must_use]
    pub fn to_dim(self, dim: &[u32]) -> Type {
        let mut ty = self;
        for len in dim.iter().rev().copied() {
            ty = Type::Array { ty: Box::new(ty), len };
        }
        ty
    }
}

pub trait AsType {
    fn as_type(&self) -> Type;
}

impl AsType for ast::Type {
    fn as_type(&self) -> Type {
        if self.integer_token().is_some() {
            Type::Integer
        } else if self.real_token().is_some() {
            Type::Real
        } else if self.string_token().is_some() {
            Type::String
        } else {
            Type::Err
        }
    }
}

impl AsType for Option<ast::Type> {
    fn as_type(&self) -> Type {
        self.as_ref().map_or(Type::Err, AsType::as_type)
    }
}
