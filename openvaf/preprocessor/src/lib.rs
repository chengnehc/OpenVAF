//! A preprocessor that deals with macro expansion.

use std::sync::Arc;

use vfs::{FileId, FileReadError, VfsPath};

#[cfg(test)]
#[rustfmt::skip]
mod tests;
mod grammar;
mod parser;
mod processor;
mod scoped_arena;

pub mod errors;
pub mod sourcemap;

use errors::PreprocessError;
use processor::Processor;
use scoped_arena::ScopedArena;
use sourcemap::{CtxSpan, SourceMap};
// use tracing::trace_span;

/// A `Token` with source context span information
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct Token {
    pub span: CtxSpan,
    pub kind: tokens::SyntaxKind,
}

#[derive(PartialEq, Eq, Clone, Debug)]
pub struct Preprocess {
    pub tokens: Arc<Vec<Token>>,
    pub errors: Arc<Vec<PreprocessError>>,
    pub source_map: Arc<SourceMap>,
}

impl Preprocess {
    pub fn errors(&self) -> &[PreprocessError] {
        &self.errors
    }
}

type Text = Arc<str>;
type ScopedTextArena = ScopedArena<Text>;

/// # Panics
/// This function panics if called multiple times in the same OpenVAF session
pub fn preprocess(sources: &dyn SourceProvider, file: FileId) -> Preprocess {
    // let span = trace_span!("preprocessor", main_file = display(sources.file_path(file)));
    // let _scope = span.enter();
    let storage = ScopedTextArena::new();
    let (tokens, errors, source_map) = match Processor::new(&storage, file, sources) {
        Ok(mut processor) => {
            let (tokens, errors) = processor.run(file);
            (tokens, errors, processor.source_map)
        }
        Err(FileReadError::Io(error)) => (
            vec![],
            vec![PreprocessError::FileNotFound {
                file: sources.file_path(file).to_string(),
                error,
                span: None,
            }],
            SourceMap::new(file, 0.into()),
        ),
        Err(FileReadError::InvalidTextFormat(err)) => (
            vec![],
            vec![PreprocessError::InvalidTextFormat {
                file: sources.file_path(file),
                span: None,
                err,
            }],
            SourceMap::new(file, 0.into()),
        ),
    };

    Preprocess {
        tokens: Arc::new(tokens),
        errors: Arc::new(errors),
        source_map: Arc::new(source_map),
    }
}

pub trait SourceProvider {
    fn include_dirs(&self, root_file: FileId) -> Arc<[VfsPath]>;
    fn macro_flags(&self, file_root: FileId) -> Arc<[Text]>;

    fn file_text(&self, file: FileId) -> Result<Text, FileReadError>;
    fn file_path(&self, file: FileId) -> VfsPath;
    fn file_id(&self, path: VfsPath) -> FileId;
}
