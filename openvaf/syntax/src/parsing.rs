//! This module serves as the bridge to `parser` crate (which does the actual parsing)
//! and build the SyntaxTree.

use preprocessor::sourcemap::SourceContextId;
use preprocessor::{Preprocess, SourceProvider};
use rowan::{GreenNode, TextRange, TextSize};
use vfs::FileId;

use crate::{SyntaxError /*SyntaxKind*/};

mod tree_builder;
use tree_builder::SyntaxTreeBuilder;

pub(crate) fn parse_text(
    sources: &dyn SourceProvider,
    root_file: FileId,
    Preprocess { tokens, source_map, .. }: &Preprocess,
) -> (GreenNode, Vec<SyntaxError>, Vec<(TextRange, SourceContextId, TextSize)>) {
    // initialize tree builder
    let mut builder = SyntaxTreeBuilder::new(sources, root_file, tokens, source_map);
    // filter out trivia: whitespaces/comments
    let tokens: Vec<_> = tokens
        .iter()
        .filter_map(|token| token.kind.is_non_trivia().then_some(token.kind))
        .collect();
    // parse and build
    for step in parser::parse(&tokens).iter() {
        match step {
            parser::Step::Token { kind } => builder.token(kind),
            parser::Step::Enter { kind } => builder.start_node(kind),
            parser::Step::Exit => builder.finish_node(),
            parser::Step::Error { err } => builder.error(err.clone()),
        }
    }
    let (tree, errors, ctx_map) = builder.finish();

    (tree, errors, ctx_map)
}
