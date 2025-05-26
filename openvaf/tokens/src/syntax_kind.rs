// These clippy lints are nice normally, but why generate macros
// when you can generate pre-expanded code? The `manual_non_exhaustive`
// one is a false positive.

#[allow(clippy::match_like_matches_macro, clippy::manual_non_exhaustive, clippy::enum_variant_names)]
#[rustfmt::skip]
#[macro_use]
mod generated;

pub use self::generated::SyntaxKind;

impl From<u16> for SyntaxKind {
    #[inline]
    fn from(d: u16) -> SyntaxKind {
        assert!(d <= (SyntaxKind::__LAST as u16));
        unsafe { std::mem::transmute::<u16, SyntaxKind>(d) }
    }
}

impl SyntaxKind {
    #[inline]
    pub fn is_trivia(self) -> bool {
        matches!(self, SyntaxKind::WHITESPACE | SyntaxKind::COMMENT)
    }

    #[inline]
    pub fn is_non_trivia(self) -> bool {
        !self.is_trivia()
    }
}
