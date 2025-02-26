//! Syntax Tree library
//!
//! The most interesting modules here are `syntax_node` (which defines concrete
//! syntax tree) and `ast` (which defines abstract syntax tree on top of the CST).
//!
//! The actual parser live in a separate `parser` crate.
//! The `parsing` module serves as the bridge and build the syntax tree.
//!
//! See Also:
//! - https://github.com/rust-lang/rust-analyzer/tree/master/crates/syntax
//! - https://docs.rs/ra_ap_syntax/latest/ra_ap_syntax/index.html
//! - https://github.com/rust-lang/rfcs/pull/2256
//! - https://github.com/apple/swift/blob/13d593df6f359d0cb2fc81cfaac273297c539455/lib/Syntax/README.md

use std::cmp::Ordering;
use std::marker::PhantomData;
use std::sync::Arc;
use stdx::format_to;

use preprocessor::sourcemap::{CtxSpan, FileSpan, SourceContextId};
use vfs::FileId;

pub use preprocessor::diagnostics::PreprocessError;
pub use preprocessor::sourcemap::{self, SourceMap};
pub use preprocessor::{preprocess, Preprocess, SourceProvider};
pub use rowan::{
    Direction, GreenNode, NodeOrToken, /*SyntaxText,*/ TextRange, TextSize, WalkEvent,
};
pub use tokens::{SyntaxKind, T};

mod error;
mod parsing;
mod ptr;
mod syntax_node;
mod token_text;
mod validation;

pub mod ast;
pub mod name;

pub use ast::{AstNode, SourceFile};
pub use error::SyntaxError;
pub use ptr::{AstPtr, SyntaxNodePtr};
pub use syntax_node::{SyntaxNode, SyntaxToken};
pub use token_text::TokenText;

/// `Parse` is the result of the parsing: a syntax tree and a collection of errors.
///
/// `ctx_map` is used for diagnostics and reporting.
#[derive(Debug, PartialEq, Eq)]
pub struct Parse<T> {
    green: GreenNode,
    errors: Option<Arc<[SyntaxError]>>,
    pub ctx_map: Arc<Vec<(TextRange, SourceContextId, TextSize)>>,
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
    fn new(
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

    pub fn syntax_node(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn errors(&self) -> Vec<SyntaxError> {
        let mut errors = if let Some(e) = self.errors.as_deref() { e.to_vec() } else { vec![] };
        validation::validate(&self.syntax_node(), &mut errors);
        errors
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

    pub fn to_file_span(&self, range: TextRange, sm: &SourceMap) -> FileSpan {
        self.to_ctx_span(range, sm).to_file_span(sm)
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
            .expect("No range in the sourcemap covers the requested position")
    }
}

impl<T: AstNode> Parse<T> {
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
    pub fn tree(&self) -> T {
        T::cast(self.syntax_node()).unwrap()
    }

    /// Converts from `Parse<T>` to [`Result<T, Vec<SyntaxError>>`].
    pub fn ok(self) -> Result<T, Vec<SyntaxError>> {
        match self.errors() {
            errors if !errors.is_empty() => Err(errors),
            _ => Ok(self.tree()),
        }
    }
}

/* JW: not used
impl Parse<SyntaxNode> {
    pub fn cast<N: AstNode>(self) -> Option<Parse<N>> {
        if N::cast(self.syntax_node()).is_some() {
            Some(Parse {
                green: self.green,
                errors: self.errors,
                ctx_map: self.ctx_map,
                _ty: PhantomData,
            })
        } else {
            None
        }
    }
}
*/

impl Parse<SourceFile> {
    pub fn debug_dump(&self) -> String {
        let mut buf = format!("{:#?}", self.tree().syntax());
        for err in self.errors() {
            format_to!(buf, "error : {}\n", err);
        }
        buf
    }

    // TODO(JW) for incremental reparse, currently not used.

    // pub fn reparse(&self, indel: &Indel) -> Parse<SourceFile> {
    //     self.full_reparse(indel)
    // self.incremental_reparse(indel).unwrap_or_else(|| self.full_reparse(indel))
    // }

    // fn incremental_reparse(&self, indel: &Indel) -> Option<Parse<SourceFile>> {
    //     parsing::incremental_reparse(self.tree().syntax(), indel, self.errors.to_vec()).map(
    //         |(green_node, errors, _reparsed_range)| Parse {
    //             green: green_node,
    //             errors: Arc::new(errors),
    //             _ty: PhantomData,
    //         },
    //     )
    // }

    // fn full_reparse(&self, indel: &Indel) -> Parse<SourceFile> {
    //     let mut text = self.tree().syntax().text().to_string();
    //     indel.apply(&mut text);
    //     SourceFile::parse(&text)
    // }
}

/// `SourceFile` is a `AstNode` that represents a parse tree for a file.
impl SourceFile {
    pub fn parse(
        db: &dyn SourceProvider,
        root_file: FileId,
        preprocess: &Preprocess,
    ) -> Parse<SourceFile> {
        let (tree, errors, ctx_map) = parsing::parse_text(db, root_file, preprocess);
        // let root = SyntaxNode::new_root(tree.clone());
        // assert_eq!(root.kind(), SyntaxKind::SOURCE_FILE);

        Parse::new(tree, errors, ctx_map)
    }
}

/// Matches an untyped `SyntaxNode` against a typed `AstNode`
///
/// # Example:
///
/// ```ignore
/// match_ast! {
///     match node {
///         ast::CallExpr(it) => { ... },
///         ast::MethodCallExpr(it) => { ... },
///         ast::MacroCall(it) => { ... },
///         _ => None,
///     }
/// }
/// ```
#[macro_export]
macro_rules! match_ast {
    (match $node:ident { $($tt:tt)* }) => { match_ast!(match ($node) { $($tt)* }) };

    (match ($node:expr) {
        $( ast::$ast:ident($it:ident) => $res:expr, )*
        _ => $catch_all:expr $(,)?
    }) => {{
        $( if let Some($it) = ast::$ast::cast($node.clone()) { $res } else )*
        { $catch_all }
    }};
}
