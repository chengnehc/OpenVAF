use stdx::{impl_debug_display, impl_idx_from};

use tokens::{SyntaxKind, TextRange, TextSize};
use typed_index_collections::TiVec;

use crate::sourcemap::CtxSpan;

#[derive(Debug, Clone)]
pub(crate) struct Macro<'s> {
    pub head: TextSize,
    pub span: CtxSpan,
    pub body: Vec<ParsedToken<'s>>,
    pub arg_cnt: usize,
}

impl Macro<'_> {
    pub fn head_span(&self) -> CtxSpan {
        self.span.with_len(self.head)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParsedToken<'s> {
    pub(crate) range: TextRange,
    pub(crate) kind: ParsedTokenKind<'s>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ParsedTokenKind<'s> {
    ResolvedToken(SyntaxKind),
    ArgumentReference(MacroArg),
    MacroCall(MacroCall<'s>),
}

impl From<SyntaxKind> for ParsedTokenKind<'static> {
    fn from(value: SyntaxKind) -> ParsedTokenKind<'static> {
        ParsedTokenKind::ResolvedToken(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MacroCall<'s> {
    pub name: &'s str,
    pub arg_bindings: MacroArgs<'s>,
}

pub(crate) type MacroArgs<'s> = TiVec<MacroArg, (Vec<ParsedToken<'s>>, TextRange)>;

#[derive(PartialEq, Eq, PartialOrd, Ord, Clone, Copy, Hash)]
pub(crate) struct MacroArg(u8);
impl_idx_from!(MacroArg(u8));
impl_debug_display!(c@MacroArg => "arg{}", c.0);
