//! See Also:
//!
//! https://docs.rs/ra_ap_hir_ty/latest/ra_ap_hir_ty/

pub mod builtin;
pub mod db;
pub mod inference;
pub mod lower;
pub mod types;
pub mod validation;

use builtin::BuiltinInfo;
use db::HirTyDB;
use inference::Inference;
// use lower::{BranchTy, DisciplineTy, NatureTy};
