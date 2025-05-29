pub use text_size::{TextLen, TextRange, TextSize};

mod syntax_kind;
mod token_kind;

pub use syntax_kind::SyntaxKind;
pub use token_kind::{LiteralKind, TokenKind};

/// Lexed token.
///
/// It doesn't contain information about data that has been lexed,
/// only the kind and size of the token.
#[derive(Debug, Clone, Copy)]
pub struct Token {
    pub kind: TokenKind,
    pub len: TextSize,
}

pub enum LexerError {
    UnterminatedBlockComment,
    UnterminatedStr,
    UnexpectedToken,
}

impl TokenKind {
    /// Convert this `TokenKind` with identifier `src` to corresponding `SyntaxKind`
    /// if possible, and emit lexer errors.
    pub fn to_syntax(self, src: &str) -> (Option<SyntaxKind>, Option<LexerError>) {
        let token = match self {
            // Combined operators
            Self::LineComment | Self::BlockComment { terminated: true } => SyntaxKind::COMMENT,
            Self::BlockComment { terminated: false } => {
                return (Some(SyntaxKind::COMMENT), Some(LexerError::UnterminatedBlockComment))
            }
            Self::Whitespace => SyntaxKind::WHITESPACE,
            Self::SimpleIdent => SyntaxKind::from_keyword(src).unwrap_or(SyntaxKind::IDENT),
            Self::EscapedIdent => SyntaxKind::IDENT,
            Self::SystemCallIdent if src == "$root" => SyntaxKind::ROOT_KW,
            Self::SystemCallIdent => SyntaxKind::SYSFUN,
            Self::Literal { kind: LiteralKind::Int } => SyntaxKind::INT_NUMBER,
            Self::Literal { kind: LiteralKind::Float { has_scale_char: true } } => {
                SyntaxKind::SI_REAL_NUMBER
            }
            Self::Literal { kind: LiteralKind::Float { has_scale_char: false } } => {
                SyntaxKind::STD_REAL_NUMBER
            }
            Self::Literal { kind: LiteralKind::Str { terminated: true } } => SyntaxKind::STR_LIT,
            Self::Literal { kind: LiteralKind::Str { terminated: false } } => {
                return (Some(SyntaxKind::STR_LIT), Some(LexerError::UnterminatedStr))
            }
            Self::CompilerDirective | Self::Define { .. } | Self::IllegalDefine => {
                return (None, None)
            }
            Self::Semi => T![;],
            Self::Comma => T![,],
            Self::Dot => T![.],
            Self::OpenParen => T!['('],
            Self::CloseParen => T![')'],
            Self::OpenBrace => T!['{'],
            Self::CloseBrace => T!['}'],
            Self::OpenBracket => T!['['],
            Self::CloseBracket => T![']'],
            Self::At => T![@],
            Self::Pound => T![#],
            Self::Tilde => T![~],
            Self::Question => T![?],
            Self::Colon => T![:],
            Self::Dollar => T![$],
            Self::Eq => T![=],
            Self::Not => T![!],
            Self::Lt => T![<],
            Self::Gt => T![>],
            Self::Minus => T![-],
            Self::And => T![&],
            Self::Or => T![|],
            Self::Plus => T![+],
            Self::Star => T![*],
            Self::Slash => T![/],
            Self::Caret => T![^],
            Self::Percent => T![%],
            Self::AttrOpenParen => T!["(*"],
            Self::AttrCloseParen => T!["*)"],
            Self::ArrStart => T!["'{"],
            Self::Eq2 => T![==],
            Self::Neq => T![!=],
            Self::Leq => T![<=],
            Self::Geq => T![>=],
            Self::Pipe2 => T![||],
            Self::Amp2 => T![&&],
            Self::Shl => T![<<],
            Self::Shr => T![>>],
            Self::ShlA => T![<<<],
            Self::ShrA => T![>>>],
            Self::Contribute => T![<+],
            Self::Pow => T![**],
            Self::NXorL => T![~^],
            Self::NXorR => T![^~],

            Self::Unknown => return (None, Some(LexerError::UnexpectedToken)),
        };

        (Some(token), None)
    }
}
