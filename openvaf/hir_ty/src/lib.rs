//! Type inference, i.e. the process of walking through the code and determining
//! the type of each expression and pattern.
//!
//! See Also:
//!
//! https://docs.rs/ra_ap_hir_ty/latest/ra_ap_hir_ty/

pub mod builtin;
pub mod db;
pub mod inference;
pub mod lower;
pub mod types;
pub mod validation;

pub use lower::{BranchTy, DisciplineTy, NatureTy};
