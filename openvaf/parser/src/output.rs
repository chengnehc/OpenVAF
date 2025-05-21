use crate::{Error, SyntaxKind};

/// Output of the parser -- a DFS traversal of a concrete syntax tree.
///
/// In a sense, this is just a sequence of [`SyntaxKind`]
#[derive(Default)]
pub struct Output {
    /// 32-bit encoding of events. If LSB is zero, then that's an index into the error
    /// vector. Otherwise, it's one of the three other variants, with data encoded as:
    ///
    ///     |16 bit kind|8 bit leftovers|4 bit tag|4 bit leftover|
    ///
    events: Vec<u32>,
    errors: Vec<Error>,
}

#[derive(Debug)]
pub enum Step<'a> {
    Token { kind: SyntaxKind }, // tag = 0000
    Enter { kind: SyntaxKind }, // tag = 0001
    Exit,                       // tag = 0010
    Error { err: &'a Error },
}

impl Output {
    // Transform the parser output encoding into syntax tree building steps
    pub fn iter(&self) -> impl Iterator<Item = Step<'_>> {
        self.events.iter().map(|&event| {
            if event & 0b1 == 0 {
                return Step::Error { err: &self.errors[(event as usize) >> 1] };
            }
            let tag = ((event & 0x0000_00F0) >> 4) as u8;
            match tag {
                0 => {
                    let kind: SyntaxKind = (((event & 0xFFFF_0000) >> 16) as u16).into();
                    Step::Token { kind }
                }
                1 => {
                    let kind: SyntaxKind = (((event & 0xFFFF_0000) >> 16) as u16).into();
                    Step::Enter { kind }
                }
                2 => Step::Exit,
                _ => unreachable!(),
            }
        })
    }

    pub(crate) fn token(&mut self, kind: SyntaxKind) {
        let e = ((kind as u16 as u32) << 16) | 1;
        self.events.push(e)
    }

    pub(crate) fn enter_node(&mut self, kind: SyntaxKind) {
        let e = ((kind as u16 as u32) << 16) | (1 << 4) | 1;
        self.events.push(e)
    }

    pub(crate) fn leave_node(&mut self) {
        let e = (2 << 4) | 1;
        self.events.push(e)
    }

    pub(crate) fn error(&mut self, error: Error) {
        let idx = self.errors.len();
        self.errors.push(error);
        let e = (idx as u32) << 1;
        self.events.push(e);
    }
}
