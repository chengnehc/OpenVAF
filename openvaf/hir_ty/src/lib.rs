//! See Also:
//!
//! https://docs.rs/ra_ap_hir_ty/latest/ra_ap_hir_ty/

pub mod builtin;
pub mod db;
pub mod inference;
pub mod types;
pub mod validation;

mod lower;

pub use lower::BranchKind;
