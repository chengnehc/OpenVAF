use stdx::impl_display;

use syntax::TextRange;

use crate::ErasedAstId;

#[derive(Debug, Eq, PartialEq, Clone)]
pub enum LintAttrDiagnostic {
    ExpectedArrayOrLiteral { range: TextRange, attr: &'static str },
    ExpectedLiteral { range: TextRange, attr: &'static str },
    UnknownLint { range: TextRange, lint: String, src: ErasedAstId },
    LintOverwrite { old: TextRange, new: TextRange, name: String, src: ErasedAstId },
}

use LintAttrDiagnostic::*;
impl_display! {
    match LintAttrDiagnostic {
        ExpectedArrayOrLiteral {attr,..} => "'{attr}' attribute expects a string literal or and array of literals";
        ExpectedLiteral {attr,..} => "'{attr}' attribute expects a string literal here";
        UnknownLint {lint,..} => "unknown lint '{lint}'";
        LintOverwrite {name,..} => "lint level for '{name}' was set multiple times";
    }
}
