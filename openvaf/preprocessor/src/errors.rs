use std::io;
use stdx::impl_display;

use vfs::{InvalidTextFormatErr, VfsPath};

use crate::sourcemap::CtxSpan;

#[derive(Debug, PartialEq, Clone, Eq)]
pub enum PreprocessError {
    /* Lexer */
    UnexpectedToken(CtxSpan),
    UnexpectedEof { expected: &'static str, span: CtxSpan },

    /* File System */
    FileNotFound { file: String, error: io::ErrorKind, span: Option<CtxSpan> },
    InvalidTextFormat { span: Option<CtxSpan>, file: VfsPath, err: InvalidTextFormatErr },

    /* Compiler Directive and Macros */
    UnsupportedCompDir { name: String, span: CtxSpan },
    MissingOrUnexpectedToken { expected: &'static str, expected_at: CtxSpan, found_at: CtxSpan },
    MacroNotFound { name: String, span: CtxSpan },
    MacroNotDefined { name: String, span: CtxSpan },
    MacroOverwritten { old: CtxSpan, new: CtxSpan, name: String },
    MacroArgCountMismatch { expected: usize, found: usize, span: CtxSpan },
    MacroRecursion { name: String, span: CtxSpan },
}

impl_display! {
    match PreprocessError {
        Self::UnexpectedToken(_) => "encountered unexpected token";
        Self::UnexpectedEof { expected, .. } => "unexpected EOF, expected {}", expected;
        Self::FileNotFound { file, error, .. } => "failed to read '{file}': {}", std::io::Error::from(*error);
        Self::InvalidTextFormat { file, .. } => "failed to read {file}: file contents are not valid text";
        Self::UnsupportedCompDir { name,.. } => "unsupported compiler directive {name}";
        Self::MissingOrUnexpectedToken { expected, .. } => "unexpected token, expected '{}'", expected;
        Self::MacroNotFound{ name, .. } =>  "macro '`{name}' has not been declared";
        Self::MacroNotDefined{ name, .. } =>  "cannot undefine macro '`{name}'";
        Self::MacroOverwritten { name, .. } => "macro '`{name}' was overwritten";
        Self::MacroArgCountMismatch { expected, found, .. } => "argument mismatch, expected {} but found {}", expected, found;
        Self::MacroRecursion { name, .. } => "macro '`{name}' was called recursively";
    }
}
