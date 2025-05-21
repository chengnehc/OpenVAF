//! This module serves as the bridge to `parser` crate (which does the actual parsing)
//! and build the SyntaxTree.

use std::cmp::Ordering;
use std::marker::PhantomData;
use std::sync::Arc;

use preprocessor::sourcemap::{CtxSpan, FileSpan, SourceContextId, SourceMap};
use preprocessor::{Preprocess, SourceProvider, Token};
use rowan::{GreenNode, TextRange, TextSize};
use vfs::FileId;

use crate::{validation, AstNode, SourceFile, SyntaxError, SyntaxNode};

mod reparse;
mod tree_builder;
use tree_builder::SyntaxTreeBuilder;

/// Call the `parse()` function in the `parser` crate and make use of
/// the tree builder to build AST.
pub fn parse(
    db: &dyn SourceProvider,
    root_file: FileId,
    Preprocess { tokens, source_map, .. }: &Preprocess,
) -> Parse<SourceFile> {
    let mut builder = SyntaxTreeBuilder::new(db, root_file, tokens, source_map);

    // filter out trivia(whitespaces/comments)
    let tokens: Vec<_> = tokens
        .iter()
        .filter_map(|token| token.kind.is_non_trivia().then_some(token.kind))
        .collect();

    // parse and build AST
    for step in parser::parse(&tokens).iter() {
        match step {
            parser::Step::Enter { kind } => builder.start_node(kind),
            parser::Step::Token { kind } => builder.token(kind),
            parser::Step::Error { err } => builder.error(err.clone()),
            parser::Step::Exit => builder.finish_node(),
        }
    }
    let (green, errors, ctx_map) = builder.finish();

    Parse::new(green, errors, ctx_map)
}

/// `Parse` contains the result of the parsing: a syntax tree, a collection
/// of errors, and a context mapping used for diagnostics and error reporting.
#[derive(Debug, PartialEq, Eq)]
pub struct Parse<T> {
    green: GreenNode,
    errors: Option<Arc<[SyntaxError]>>,
    ctx_map: Arc<Vec<(TextRange, SourceContextId, TextSize)>>,
    _ty: PhantomData<fn() -> T>,
}

impl<T> Clone for Parse<T> {
    fn clone(&self) -> Parse<T> {
        Parse {
            green: self.green.clone(),
            errors: self.errors.clone(),
            ctx_map: self.ctx_map.clone(),
            _ty: PhantomData,
        }
    }
}

impl<T> Parse<T> {
    pub(crate) fn new(
        green: GreenNode,
        errors: Vec<SyntaxError>,
        ctx_map: Vec<(TextRange, SourceContextId, TextSize)>,
    ) -> Parse<T> {
        Parse {
            green,
            errors: if errors.is_empty() { None } else { Some(errors.into()) },
            ctx_map: Arc::new(ctx_map),
            _ty: PhantomData,
        }
    }

    /// Return the root node of the parsed syntax tree.
    #[inline]
    pub fn root(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn errors(&self) -> Vec<SyntaxError> {
        let mut errors = if let Some(e) = self.errors.as_deref() { e.to_vec() } else { vec![] };
        validation::validate(&self.root(), &mut errors);
        errors
    }

    #[inline]
    pub fn to_file_span(&self, range: TextRange, sm: &SourceMap) -> FileSpan {
        self.to_ctx_span(range, sm).to_file_span(sm)
    }

    pub fn to_ctx_span(&self, range: TextRange, sm: &SourceMap) -> CtxSpan {
        let (ctx_range1, ctx1, offset1) = self.find_ctx_range(range.start());
        if range.end() <= ctx_range1.end() {
            // Special and common case that start and end are in the same `SourceContext`
            CtxSpan { range: range - ctx_range1.start() + offset1, ctx: ctx1 }
        } else {
            // Expensive but much less common case
            let (ctx_range2, ctx2, offset2) = self.find_ctx_range(range.end());
            let start = range.start() - ctx_range1.start() + offset1;
            let end = range.end() - ctx_range2.start() + offset2;
            if ctx1 == ctx2 {
                CtxSpan { range: TextRange::new(start, end), ctx: ctx1 }
            } else {
                let (ctx, range1, range2) = sm.lowest_common_parent(
                    CtxSpan { range: TextRange::empty(start), ctx: ctx1 },
                    CtxSpan { range: TextRange::empty(end), ctx: ctx2 },
                );

                CtxSpan { range: range1.cover(range2), ctx }
            }
        }
    }

    /* JW: not used.
    pub fn ctx(&self, global_pos: TextSize) -> (SourceContextId, TextSize) {
        let (range, ctx, _offset) = self.find_ctx_range(global_pos);
        let relative_pos = global_pos - range.start();
        (ctx, relative_pos)
    }
    */

    fn find_ctx_range(&self, global_pos: TextSize) -> (TextRange, SourceContextId, TextSize) {
        self.ctx_map
            .binary_search_by(|(range, _, _)| {
                if range.end() <= global_pos {
                    Ordering::Less
                } else if global_pos < range.start() {
                    Ordering::Greater
                } else {
                    Ordering::Equal
                }
            })
            .ok()
            .map(|i| self.ctx_map[i])
            .expect("The source map should cover the requested position")
    }
}

impl<N: AstNode> Parse<N> {
    /// Erase the type of the parsed syntax tree.
    pub fn to_syntax(self) -> Parse<SyntaxNode> {
        Parse { green: self.green, errors: self.errors, ctx_map: self.ctx_map, _ty: PhantomData }
    }

    /// Gets the parsed syntax tree as a typed ast node through casting.
    ///
    /// # Panics
    ///
    /// Panics if the root node cannot be casted into the typed ast node
    /// (e.g. if it's an ERROR node).
    pub fn tree(&self) -> N {
        N::cast(self.root()).unwrap()
    }

    /// Converts from `Parse<T>` to [`Result<T, Vec<SyntaxError>>`].
    pub fn ok(self) -> Result<N, Vec<SyntaxError>> {
        match self.errors() {
            errors if !errors.is_empty() => Err(errors),
            _ => Ok(self.tree()),
        }
    }
}

impl Parse<SourceFile> {
    pub fn debug_dump(&self) -> String {
        let mut buf = format!("{:#?}", self.tree().syntax());
        for err in self.errors() {
            stdx::format_to!(buf, "error : {err}\n");
        }
        buf
    }
}
