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

use PreprocessError::*;
impl_display! {
    match PreprocessError {
        UnexpectedToken(_) => "encountered unexpected token";
        UnexpectedEof { expected, .. } => "unexpected EOF, expected {}", expected;
        FileNotFound { file, error, .. } => "failed to read '{file}': {}", std::io::Error::from(*error);
        InvalidTextFormat { file, .. } => "failed to read {file}: file contents are not valid text";
        UnsupportedCompDir { name,.. } => "unsupported compiler directive {name}";
        MissingOrUnexpectedToken { expected, .. } => "unexpected token, expected '{}'", expected;
        MacroNotFound{ name, .. } =>  "macro '`{name}' has not been declared";
        MacroNotDefined{ name, .. } =>  "cannot undefine macro '`{name}'";
        MacroOverwritten { name, .. } => "macro '`{name}' was overwritten";
        MacroArgCountMismatch { expected, found, .. } => "argument mismatch, expected {} but found {}", expected, found;
        MacroRecursion { name, .. } => "macro '`{name}' was called recursively";
    }
}
