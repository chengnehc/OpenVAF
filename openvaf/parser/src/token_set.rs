use std::convert::TryInto;

use crate::SyntaxKind;

/// A bit-set of `SyntaxKind`s
#[derive(Clone, Copy)]
pub(crate) struct TokenSet(u128);

/// `TokenSet`s should only include token `SyntaxKind`s,
/// so the discriminant of any passed/included `SyntaxKind`
/// must *not* be greater than that of the last token `SyntaxKind`.
const LAST_TOKEN_KIND_DISCRIMINANT: usize = SyntaxKind::COMMENT as usize;

impl TokenSet {
    pub(crate) const EMPTY: TokenSet = TokenSet(0);

    pub(crate) const fn new(kinds: &[SyntaxKind]) -> TokenSet {
        let mut res = 0u128;
        let mut i = 0;
        while i < kinds.len() {
            let discriminant = kinds[i] as usize;
            debug_assert!(
                discriminant <= LAST_TOKEN_KIND_DISCRIMINANT,
                "Expected a token `SyntaxKind`"
            );
            res |= mask(discriminant);
            i += 1
        }
        TokenSet(res)
    }

    pub(crate) const fn unique(kind: SyntaxKind) -> TokenSet {
        Self::new(&[kind])
    }

    pub(crate) const fn union(self, other: TokenSet) -> TokenSet {
        TokenSet(self.0 | other.0)
    }

    pub(crate) const fn contains(&self, kind: SyntaxKind) -> bool {
        let discriminant = kind as usize;
        debug_assert!(
            discriminant <= LAST_TOKEN_KIND_DISCRIMINANT,
            "Expected a token `SyntaxKind`"
        );
        self.0 & mask(discriminant) != 0
    }

    pub(crate) const fn iter(&self) -> TokenSetIter {
        TokenSetIter(self.0)
    }
}

const fn mask(kind: usize) -> u128 {
    1u128 << kind
}

pub(crate) struct TokenSetIter(u128);

impl Iterator for TokenSetIter {
    type Item = SyntaxKind;
    fn next(&mut self) -> Option<SyntaxKind> {
        if self.0 != 0 {
            let bit_pos: u16 = self.0.trailing_zeros().try_into().unwrap();
            let bit = 1 << bit_pos;
            self.0 ^= bit;
            Some(SyntaxKind::from(bit_pos))
        } else {
            None
        }
    }
}

#[test]
fn token_set_works() {
    use crate::SyntaxKind::*;
    let ts = TokenSet::new(&[EOF, COMMENT]);
    assert!(ts.contains(EOF));
    assert!(ts.contains(COMMENT));
    assert!(!ts.contains(PARAMETER_KW));
}
