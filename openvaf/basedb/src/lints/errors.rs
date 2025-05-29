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

impl_display! {
    match LintAttrDiagnostic {
        Self::ExpectedArrayOrLiteral {attr,..} => "'{attr}' attribute expects a string literal or and array of literals";
        Self::ExpectedLiteral {attr,..} => "'{attr}' attribute expects a string literal here";
        Self::UnknownLint {lint,..} => "unknown lint '{lint}'";
        Self::LintOverwrite {name,..} => "lint level for '{name}' was set multiple times";
    }
}
