use std::str::Chars;

use text_size::TextSize;
use tokens::{Token, TokenKind};

/// Peekable iterator over a char sequence.
///
/// Next characters can be peeked via `nth_char` method,
/// and position can be shifted forward via `bump` method.
pub(crate) struct Cursor<'a> {
    len_remaining: TextSize,
    // an iterator of symbols
    chars: Chars<'a>,
    // the last eaten symbol
    #[cfg(debug_assertions)]
    prev: char,
    // TODO(JW) consider refactoring this out of `Cursor`?
    dst: Vec<Token>,
    // for '`define' macro expansion
    marker: Option<usize>,
}

pub(crate) const EOF_CHAR: char = '\0';

impl<'a> Cursor<'a> {
    pub(crate) fn new(input: &'a str) -> Cursor<'a> {
        Cursor {
            len_remaining: TextSize::of(input),
            chars: input.chars(),
            #[cfg(debug_assertions)]
            prev: EOF_CHAR,
            // Tokens are on averge a length of about 4
            dst: Vec::with_capacity(input.len() / 4),
            marker: None,
        }
    }

    /// Returns the last eaten symbol in debug builds, or `'\0'` in release builds.
    pub(crate) fn prev(&self) -> char {
        #[cfg(debug_assertions)]
        {
            self.prev
        }

        #[cfg(not(debug_assertions))]
        {
            EOF_CHAR
        }
    }

    /// Peeks the next symbol from the input stream without consuming it.
    /// If requested position doesn't exist, `EOF_CHAR` is returned.
    ///
    /// However, getting `EOF_CHAR` doesn't always mean actual end of file,
    /// it should be checked with `is_eof` method.
    pub fn first(&self) -> char {
        // `.next()` optimizes better than `.nth(0)`
        self.chars.clone().next().unwrap_or(EOF_CHAR)
    }

    /// Peeks the second symbol from the input stream without consuming it.
    pub(crate) fn second(&self) -> char {
        // `.next()` optimizes better than `.nth(1)`
        let mut iter = self.chars.clone();
        iter.next();
        iter.next().unwrap_or(EOF_CHAR)
    }

    /// Checks if there is nothing more to consume.
    pub(crate) fn is_eof(&self) -> bool {
        self.chars.as_str().is_empty()
    }

    /// Returns amount of already consumed symbols.
    pub(crate) fn pos_within_token(&self) -> TextSize {
        self.len_remaining - TextSize::of(self.chars.as_str())
    }

    /// Resets the number of bytes consumed to 0.
    pub(crate) fn reset_pos_within_token(&mut self) {
        self.len_remaining = TextSize::of(self.chars.as_str());
    }

    /// Moves to the next character.
    pub(crate) fn bump(&mut self) -> Option<char> {
        let c = self.chars.next()?;

        #[cfg(debug_assertions)]
        {
            self.prev = c;
        }

        Some(c)
    }

    pub(crate) fn finish_token(&mut self, kind: TokenKind) {
        let len = self.pos_within_token();
        self.reset_pos_within_token();
        self.dst.push(Token { kind, len })
    }

    pub(crate) fn set_marker(&mut self) -> TokenKind {
        // Nested define statements are not allowed.
        if self.marker.is_none() {
            self.marker = Some(self.dst.len())
        }
        // Placeholder that remains if the marker cannot be finished.
        TokenKind::IllegalDefine
    }

    pub(crate) fn finish_marker(&mut self) -> bool {
        if let Some(marker) = self.marker.take() {
            self.dst[marker].kind = TokenKind::Define { end: self.dst.len() };
            true
        } else {
            false
        }
    }

    pub(crate) fn finish(mut self) -> Vec<Token> {
        self.finish_marker();
        self.dst
    }
}
