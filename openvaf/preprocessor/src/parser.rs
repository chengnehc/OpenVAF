use std::cmp::min;
use std::ops::Range;
use stdx::impl_idx_math_from;

use lexer::LexerError;
use tokens::{LiteralKind, SyntaxKind, TextRange, TextSize, TokenKind};
use typed_index_collections::{TiSlice, TiVec};
use vfs::VfsPath;

use crate::errors::PreprocessError;
use crate::macros::ParsedToken;
use crate::sourcemap::{CtxSpan, SourceContextId};

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub struct FullTokenIdx(u32);
impl_idx_math_from!(FullTokenIdx(u32));

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Debug)]
pub struct RelevantTokenIdx(u32);
impl_idx_math_from!(RelevantTokenIdx(u32));

pub(crate) struct Parser<'a, 'd> {
    src: &'a str,
    full_tokens: TiVec<FullTokenIdx, lexer::Token>,
    full_token_pos: FullTokenIdx,
    relevant_tokens: TiVec<RelevantTokenIdx, (PreprocessorToken, FullTokenIdx)>,
    pos: RelevantTokenIdx,
    offset: TextSize,
    previous_offset: TextSize,
    token: PreprocessorToken,
    pub(crate) ctx: SourceContextId,
    pub(crate) dst: &'d mut Vec<crate::Token>,
    pub(crate) cwd: VfsPath,
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum PreprocessorToken {
    Define { end: FullTokenIdx },
    StrLit,
    SimpleIdent,
    OpenParen,
    CloseParen,
    CompilerDirective,
    Comma,
    Other,
    Eof,
}

/// Refer to LRM chapter 10
#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum CompilerDirective {
    Include,
    IfDef,
    IfNotDef,
    Else,
    ElseIf,
    EndIf,
    Undef,
    ResetAll,
    Macro,
}

fn make_token(
    pos: RelevantTokenIdx,
    relevant_tokens: &TiSlice<RelevantTokenIdx, (PreprocessorToken, FullTokenIdx)>,
    file_end: FullTokenIdx,
) -> (PreprocessorToken, FullTokenIdx) {
    relevant_tokens
        .get(pos)
        .map_or((PreprocessorToken::Eof, file_end), |(token, pos)| (*token, *pos))
}

impl<'a, 'd> Parser<'a, 'd> {
    pub(crate) fn new(
        src: &'a str,
        ctx: SourceContextId,
        cwd: VfsPath,
        dst: &'d mut Vec<crate::Token>,
        err: &mut Vec<PreprocessError>,
    ) -> Self {
        let full_tokens = TiVec::from(lexer::tokenize(src));
        let mut relevant_tokens: TiVec<_, _> = full_tokens
            .iter_enumerated()
            .filter_map(|(pos, token)| {
                let token = match token.kind {
                    TokenKind::Define { end } => {
                        PreprocessorToken::Define { end: FullTokenIdx::from(end) }
                    }
                    TokenKind::SimpleIdent => PreprocessorToken::SimpleIdent,
                    TokenKind::OpenParen => PreprocessorToken::OpenParen,
                    TokenKind::CloseParen => PreprocessorToken::CloseParen,
                    TokenKind::Comma => PreprocessorToken::Comma,
                    TokenKind::CompilerDirective => PreprocessorToken::CompilerDirective,
                    TokenKind::Literal { kind: LiteralKind::Str { .. } } => {
                        PreprocessorToken::StrLit
                    }
                    TokenKind::Whitespace
                    | TokenKind::LineComment
                    | TokenKind::BlockComment { .. } => return None,
                    _ => PreprocessorToken::Other,
                };
                Some((token, pos))
            })
            .collect();

        relevant_tokens.push((PreprocessorToken::Eof, full_tokens.next_key()));
        dst.reserve(full_tokens.len());

        let (token, full_token_pos) =
            make_token(RelevantTokenIdx(0), &relevant_tokens, full_tokens.next_key());

        let mut res = Self {
            relevant_tokens,
            full_tokens,
            src,
            ctx,
            dst,
            cwd,
            previous_offset: 0.into(),
            offset: 0.into(),
            token,
            pos: RelevantTokenIdx(0),
            full_token_pos,
        };

        res.advance(true, 0u32.into(), err);
        res
    }

    pub(crate) fn before(&self, end: FullTokenIdx) -> bool {
        self.full_token_pos < end
    }

    pub(crate) fn current(&self) -> PreprocessorToken {
        self.token
    }

    pub(crate) fn current_range(&self) -> TextRange {
        TextRange::at(
            self.offset,
            self.full_tokens.get(self.full_token_pos).map_or(0.into(), |t| t.len),
        )
    }

    pub(crate) fn current_span(&self) -> CtxSpan {
        CtxSpan { range: self.current_range(), ctx: self.ctx }
    }

    pub(crate) fn current_text(&self) -> &'a str {
        &self.src[self.current_range()]
    }

    pub(crate) fn end(&self) -> FullTokenIdx {
        self.full_tokens.next_key() - 1u32
    }

    pub(crate) fn end_pos(&self, end: FullTokenIdx) -> TextSize {
        let pos = self.relevant_tokens[self.pos].1;
        let len: TextSize = self.full_tokens[end..pos].iter().map(|t| t.len).sum();
        self.offset - len
    }

    pub(crate) fn previous_range(&self) -> TextRange {
        let pos =
            self.relevant_tokens.get(self.pos - 1u32).map_or(self.full_token_pos, |(_, pos)| *pos);
        let len = self.full_tokens[pos].len;
        TextRange::at(self.previous_offset, len)
    }

    pub(crate) fn followed_by_bracket_without_space(&self) -> bool {
        let (token, idx) = self.relevant_tokens[self.pos + 1u32];
        token == PreprocessorToken::OpenParen && idx == (self.full_token_pos + 1u32)
    }

    fn do_bump(&mut self, save: bool, err: &mut Vec<PreprocessError>) {
        // trace!(token = display(self.current()), save = save_token, "bump");

        if self.token == PreprocessorToken::Eof {
            return;
        }

        self.previous_offset = self.offset;
        let start = self.full_token_pos;

        let (token, full_token_pos) =
            make_token(self.pos + 1u32, &self.relevant_tokens, self.full_tokens.next_key());
        self.token = token;
        self.full_token_pos = full_token_pos;
        self.pos += 1u32;
        self.advance(save, start, err);
    }

    fn advance(&mut self, save: bool, start: FullTokenIdx, err: &mut Vec<PreprocessError>) {
        let range = start..self.full_token_pos;
        if save {
            self.dst.extend(self.full_tokens[range].iter().filter_map(|token| {
                let res = Self::convert_lexer_token(*token, self.offset, self.src, err, self.ctx);
                self.offset += token.len;
                let (kind, range) = res?;
                Some(crate::Token { span: CtxSpan { range, ctx: self.ctx }, kind })
            }))
        } else {
            let len: TextSize = self.full_tokens[range].iter().map(|token| token.len).sum();
            self.offset += len;
        }
    }

    fn convert_lexer_token(
        token: lexer::Token,
        offset: TextSize,
        src: &str,
        err: &mut Vec<PreprocessError>,
        ctx: SourceContextId,
    ) -> Option<(SyntaxKind, TextRange)> {
        let range = TextRange::at(offset, token.len);
        let (syntax, error) = token.kind.to_syntax(&src[range]);
        if let Some(error) = error {
            let span = CtxSpan { range, ctx };
            match error {
                LexerError::UnterminatedStr => {
                    err.push(PreprocessError::UnexpectedEof { expected: "\"", span })
                }
                LexerError::UnexpectedToken => err.push(PreprocessError::UnexpectedToken(span)),
                LexerError::UnterminatedBlockComment => {
                    err.push(PreprocessError::UnexpectedEof { expected: "*/", span })
                }
            }
        }

        syntax.map(|kind| (kind, range))
    }

    fn save_tokens_to_macro(
        &mut self,
        range: Range<FullTokenIdx>,
        dst: &mut Vec<ParsedToken<'a>>,
        err: &mut Vec<PreprocessError>,
    ) {
        dst.extend(self.full_tokens[range].iter().filter_map(|token| {
            let res = Self::convert_lexer_token(*token, self.offset, self.src, err, self.ctx);
            self.offset += token.len;
            let (kind, range) = res?;
            Some(ParsedToken { kind: kind.into(), range })
        }))
    }

    pub(crate) fn bump_to_macro(
        &mut self,
        dst: &mut Vec<ParsedToken<'a>>,
        end: FullTokenIdx,
        err: &mut Vec<PreprocessError>,
    ) {
        if self.token == PreprocessorToken::Eof {
            return;
        }

        self.previous_offset = self.offset;
        let start = self.full_token_pos;

        let (token, full_token_pos) =
            make_token(self.pos + 1u32, &self.relevant_tokens, self.full_tokens.next_key());
        self.token = token;
        self.full_token_pos = full_token_pos;
        self.pos += 1u32;
        let macro_end = min(end, self.full_token_pos);
        self.save_tokens_to_macro(start..macro_end, dst, err);
        self.advance(true, macro_end, err)
    }

    pub(crate) fn expect(
        &mut self,
        token: PreprocessorToken,
        expected: &'static str,
        errors: &mut Vec<PreprocessError>,
    ) -> bool {
        if !self.eat(token) {
            // debug!("syntax error: expected {:?} but found {:?}", token, self.current());
            errors.push(PreprocessError::MissingOrUnexpectedToken {
                expected,
                expected_at: CtxSpan { range: self.previous_range(), ctx: self.ctx },
                found_at: CtxSpan { range: self.current_range(), ctx: self.ctx },
            });
            // self.eat(RawToken::Unexpected); // Only report lexer errors once
            false
        } else {
            true
        }
    }

    pub(crate) fn at(&self, token: PreprocessorToken) -> bool {
        self.current() == token
    }

    pub(crate) fn ctx(&self) -> SourceContextId {
        self.ctx
    }

    /// Consume the next token if `kind` matches.
    pub(crate) fn eat(&mut self, token: PreprocessorToken) -> bool {
        if !self.at(token) {
            return false;
        }
        self.do_bump(false, &mut Vec::new());
        true
    }

    /// Consume the next token if `kind` matches.
    pub(crate) fn bump(&mut self) {
        self.do_bump(false, &mut Vec::new())
    }

    /// Advances the parser by one token
    pub(crate) fn save_token(&mut self, err: &mut Vec<PreprocessError>) {
        self.do_bump(true, err)
    }

    pub(crate) fn compiler_directive(&self) -> CompilerDirective {
        match self.current_text() {
            "`include" => CompilerDirective::Include,
            "`ifdef" => CompilerDirective::IfDef,
            "`ifndef" => CompilerDirective::IfNotDef,
            "`else" => CompilerDirective::Else,
            "`elsif" => CompilerDirective::ElseIf,
            "`endif" => CompilerDirective::EndIf,
            "`undef" => CompilerDirective::Undef,
            "`resetall" => CompilerDirective::ResetAll,
            _ => CompilerDirective::Macro,
        }
    }
}
