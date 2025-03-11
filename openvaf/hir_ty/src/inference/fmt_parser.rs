//! [LRM 9.4]: Display system tasks
//!
//! For displaying real numbers, the foramt specifications have the full formatting capabilities
//! available in the C language (in terms of width and precision).
//!
//! See also: https://cplusplus.com/reference/cstdio/printf/

use std::str::CharIndices;

use hir_def::ExprId;
use syntax::{TextRange, TextSize};

use crate::inference::InferDiagnostic;

#[derive(PartialEq, Eq, PartialOrd, Ord, Copy, Clone)]
enum ParserState {
    Flags,
    FixedWidth,
    DynamicWidth,
    AnyPrecision,
    FixedPrecision,
    DynamicPrecsion,
}

impl ParserState {
    fn start_precision(self) -> bool {
        self < Self::AnyPrecision
    }

    fn eat_number(self) -> bool {
        matches!(self, Self::FixedPrecision | Self::FixedWidth)
    }

    fn candidates(self) -> &'static [char] {
        match self {
            ParserState::Flags => &[
                '-', '+', ' ', '#', '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '*', '.',
                'e', 'E', 'f', 'F', 'g', 'G', 'r', 'R', '%', 'd', 'D', 'h', 'H', 'o', 'O', 'b',
                'B', 'c', 'C', 'm', 'M', 'l', 'L', 's', 'S',
            ],
            ParserState::FixedWidth => &[
                '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '.', 'e', 'E', 'f', 'F', 'g',
                'G', 'r', 'R',
            ],
            ParserState::DynamicWidth => &['.', 'e', 'E', 'f', 'F', 'g', 'G', 'r', 'R'],
            ParserState::AnyPrecision => &['0', '1', '2', '3', '4', '5', '6', '7', '8', '9', '*'],
            ParserState::FixedPrecision => &[
                '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'e', 'E', 'f', 'F', 'g', 'G',
                'r', 'R',
            ],
            ParserState::DynamicPrecsion => &['e', 'E', 'f', 'F', 'g', 'G', 'r', 'R'],
        }
    }
}

pub struct ParseResult {
    pub dynamic_args: Vec<TextSize>,
    pub err: Option<InferDiagnostic>,
    pub end: TextSize,
}

pub fn parse_real_fmt_spec(
    start: u32,
    fmt_expr: ExprId,
    mut pos: Option<(usize, char)>,
    chars: &mut CharIndices,
) -> ParseResult {
    let mut state = ParserState::Flags;
    let mut end = start + 1;
    let mut dynamic_args = Vec::new();
    let mut err = None;
    loop {
        if let Some((off, c)) = pos {
            end = (off + c.len_utf8()) as u32;
            match c {
                // flags
                '-' | '+' | ' ' | '#' if state == ParserState::Flags => {}
                '0'..='9' if state == ParserState::Flags => {
                    state = ParserState::FixedWidth;
                }
                '*' if state == ParserState::Flags => {
                    dynamic_args.push(off.try_into().unwrap());
                    state = ParserState::DynamicWidth;
                }
                '.' if state.start_precision() => {
                    state = ParserState::AnyPrecision;
                }
                '0'..='9' if state == ParserState::AnyPrecision => {
                    state = ParserState::FixedPrecision
                }
                '*' if state == ParserState::AnyPrecision => {
                    dynamic_args.push(off.try_into().unwrap());
                    state = ParserState::DynamicPrecsion
                }
                '0'..='9' if state.eat_number() => (),
                'e'..='g' | 'E'..='G' | 'r' | 'R' if state != ParserState::AnyPrecision => {
                    break;
                }
                _ => {
                    err = Some(InferDiagnostic::InvalidFmtSpecifierChar {
                        fmt_lit: fmt_expr,
                        lit_range: TextRange::new(off.try_into().unwrap(), end.into()),
                        err_char: c,
                        candidates: state.candidates(),
                    });
                    break;
                }
            }
            pos = chars.next();
        } else {
            err = Some(InferDiagnostic::InvalidFmtSpecifierEnd {
                fmt_lit: fmt_expr,
                lit_range: TextRange::new(start.into(), end.into()),
            });
            break;
        }
    }

    ParseResult { dynamic_args, err, end: end.into() }
}
