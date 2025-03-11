//! A syntax tree builder wraps rowan's `GreenNodeBuilder`, while it extracts
//! richer information for diagnostics:
//!
//! 1. More detailed error messages for error nodes.
//! 2. Context span and text ranges mapping of each node.

use std::mem;
use std::sync::Arc;

use preprocessor::sourcemap::{CtxSpan, SourceContextId, SourceMap};
use preprocessor::{SourceProvider, Token};
use rowan::{GreenNode, GreenNodeBuilder, Language};
use vfs::FileId;

use crate::syntax_node::VerilogALanguage;
use crate::{SyntaxError, SyntaxKind, TextRange, TextSize, T};

type Text = Arc<str>;

pub(crate) struct SyntaxTreeBuilder<'a> {
    db: &'a dyn SourceProvider,
    inner: GreenNodeBuilder<'static>,
    state: State,

    tokens: &'a [Token],
    text_pos: TextSize,
    token_pos: usize,
    current_span: CtxSpan,

    errors: Vec<SyntaxError>,
    last_error: Option<SyntaxError>,
    err_depth: u32,
    panic: bool,

    sm: &'a SourceMap,
    current_text: Text,
    /// (context range, context id, offset)
    ranges: Vec<(TextRange, SourceContextId, TextSize)>,
}

enum State {
    PendingStart,
    Normal,
    PendingFinish,
}

impl<'a> SyntaxTreeBuilder<'a> {
    pub(super) fn new(
        db: &'a dyn SourceProvider,
        root_file: FileId,
        tokens: &'a [Token],
        sm: &'a SourceMap,
    ) -> Self {
        let current_text = db.file_text(root_file).unwrap_or_else(|_| Arc::from(""));
        Self {
            db,
            inner: Default::default(),
            state: State::PendingStart,
            tokens,
            text_pos: 0.into(),
            token_pos: 0,
            panic: false,
            err_depth: u32::MAX,
            errors: Vec::new(),
            last_error: None,
            sm,
            current_text,
            current_span: CtxSpan {
                ctx: SourceContextId::ROOT,
                range: TextRange::empty(TextSize::from(0)),
            },
            ranges: Vec::with_capacity(128),
        }
    }

    pub(super) fn finish(
        mut self,
    ) -> (GreenNode, Vec<SyntaxError>, Vec<(TextRange, SourceContextId, TextSize)>) {
        match mem::replace(&mut self.state, State::Normal) {
            State::PendingFinish => {
                self.eat_trivia();
                self.inner.finish_node()
            }
            State::PendingStart | State::Normal => unreachable!(),
        }
        let start = self.ranges.last().map_or(0.into(), |(range, _, _)| range.end());
        let range = TextRange::new(start, self.text_pos);
        self.ranges.push((range, self.current_span.ctx, self.current_span.range.start()));

        (self.inner.finish(), self.errors, self.ranges)
    }

    pub(super) fn token(&mut self, kind: SyntaxKind) {
        match mem::replace(&mut self.state, State::Normal) {
            State::PendingStart => unreachable!(),
            State::PendingFinish => self.inner.finish_node(),
            State::Normal => (),
        }
        self.eat_trivia();
        self.panic &= !matches!(
            kind,
            T![;] | T![end] | T![endnature] | T![endmodule] | T![enddiscipline] | T![endfunction]
        ) || self.err_depth != u32::MAX;
        let span = self.tokens[self.token_pos].span;
        self.do_token(kind, span);
    }

    pub(super) fn start_node(&mut self, kind: SyntaxKind) {
        match mem::replace(&mut self.state, State::Normal) {
            State::PendingStart => {
                self.inner.start_node(VerilogALanguage::kind_to_raw(kind));
                return; // No need to attach trivia to previous node: there is no previous node.
            }
            State::PendingFinish => self.inner.finish_node(),
            State::Normal => (),
        }

        if self.err_depth != u32::MAX {
            self.err_depth += 1
        } else if kind == SyntaxKind::ERROR {
            self.err_depth = 0
        } else {
            self.eat_trivia();
        }
        self.inner.start_node(VerilogALanguage::kind_to_raw(kind));
    }

    pub(super) fn finish_node(&mut self) {
        match mem::replace(&mut self.state, State::PendingFinish) {
            State::PendingStart => unreachable!(),
            State::PendingFinish => self.inner.finish_node(),
            State::Normal => (),
        }
        if self.err_depth == 0 {
            if let Some(mut err) = self.last_error.take() {
                if let SyntaxError::UnexpectedToken { range, panic_end, .. } = &mut err {
                    if range.end() < self.text_pos {
                        *panic_end = Some(self.text_pos);
                    }
                }
                self.errors.push(err)
            }
            self.err_depth = u32::MAX;
        } else if self.err_depth != u32::MAX {
            self.err_depth -= 1;
        }
    }

    /// Turn a parser error into a `SyntaxError` with richer diagnostics information
    pub(super) fn error(&mut self, error: parser::Error) {
        let parser::Error::UnexpectedToken { expected, found } = error;
        let missing_delimiter = found == T![end];
        let expected_at = expected
            .data
            .iter()
            .any(|t| *t == T![;] || *t == T![')'])
            .then(|| TextRange::at(self.text_pos, 0.into()));
        let n_trivia =
            self.tokens[self.token_pos..].iter().take_while(|it| it.kind.is_trivia()).count();

        // everything that follows is trivia
        if self.tokens.len() == self.token_pos + n_trivia {
            let error = SyntaxError::UnexpectedToken {
                expected,
                found,
                range: TextRange::at(
                    self.text_pos,
                    self.tokens.last().map_or_else(|| TextSize::from(0), |t| t.span.range.len()),
                ),
                expected_at,
                missing_delimiter,
                panic_end: None,
            };
            self.errors.push(error);
            return;
        }

        // see if we should panic abort
        let panic = mem::replace(&mut self.panic, true);
        if panic && !missing_delimiter || self.last_error.is_some() {
            return;
        }

        let trivia = &self.tokens[self.token_pos..self.token_pos + n_trivia];
        let offset: TextSize = trivia.iter().map(|it| it.span.range.len()).sum();
        let len = self.tokens[self.token_pos + n_trivia].span.range.len();
        let error = SyntaxError::UnexpectedToken {
            expected,
            found,
            range: TextRange::at(self.text_pos + offset, len),
            expected_at,
            missing_delimiter,
            panic_end: None,
        };
        self.last_error = Some(error)
    }

    fn eat_trivia(&mut self) {
        while let Some(&token) = self.tokens.get(self.token_pos) {
            if token.kind.is_trivia() {
                self.do_token(token.kind, token.span);
            } else {
                break;
            }
        }
    }

    fn do_token(&mut self, kind: SyntaxKind, span: CtxSpan) {
        let is_same_ctxt = span.ctx == self.current_span.ctx;
        let is_continuous = is_same_ctxt && span.range.start() == self.current_span.range.end();

        if is_continuous {
            self.current_span.range = self.current_span.range.cover(span.range);
        } else {
            let start = self.ranges.last().map_or(0.into(), |(range, _, _)| range.end());
            let range = TextRange::new(start, self.text_pos);
            self.ranges.push((range, self.current_span.ctx, self.current_span.range.start()));
            self.current_span = span;
        }

        if !is_same_ctxt {
            // The source text comes from somewhere else, so context switch is needed.
            // Unwrap is okay here because the file was already read successfully by the
            // preprocessor, otherwise the SourceContext wouldn't exist.
            let decl = self.sm.ctx_data(span.ctx).decl;
            let text = self.db.file_text(decl.file).unwrap();
            self.current_text = text;
        }

        let range = span.to_file_span(self.sm).range;
        let text = &self.current_text[range];
        self.inner.token(VerilogALanguage::kind_to_raw(kind), text);
        self.text_pos += range.len();
        self.token_pos += 1;
    }
}
