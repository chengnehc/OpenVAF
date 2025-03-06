use std::cell::Cell;
use stdx::pretty::List;

use drop_bomb::DropBomb;

use crate::event::Event;
use crate::token_set::TokenSet;
use crate::Error;
use crate::SyntaxKind::{self, EOF, ERROR, TOMBSTONE};

/// `Parser` struct provides the low-level API for
/// navigating through the stream of tokens and
/// constructing the parse tree. The actual parsing
/// happens in the `grammar` module.
///
/// However, the result of this `Parser` is not a real
/// tree, but rather a flat stream of events of the form
/// "start expression, consume number literal,
/// finish expression". See `Event` docs for more.
pub(crate) struct Parser<'t> {
    tokens: &'t [SyntaxKind],
    pos: u32,
    events: Vec<Event>,
    steps: Cell<u32>,
}

const PARSER_STEP_LIMIT: usize = 10_000_000;

impl<'t> Parser<'t> {
    pub(super) fn new(tokens: &'t [SyntaxKind]) -> Parser<'t> {
        Parser { tokens, pos: 0, events: Vec::new(), steps: Cell::new(0) }
    }

    pub(crate) fn finish(self) -> Vec<Event> {
        self.events
    }

    /// Returns the kind of the current token.
    /// If parser has already reached the end of input,
    /// the special `EOF` kind is returned.
    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    /// Lookahead operation: returns the kind of the next nth token.
    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        assert!(n <= 3);

        let steps = self.steps.get();
        assert!((steps as usize) < PARSER_STEP_LIMIT, "the parser seems stuck");
        self.steps.set(steps + 1);

        self.tokens.get(self.pos as usize + n).copied().unwrap_or(EOF)
    }

    /// Checks if the current token matches `kind`.
    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.nth_at(0, kind)
    }

    /// Checks if the nth token matches `kind`.
    pub(crate) fn nth_at(&self, n: usize, kind: SyntaxKind) -> bool {
        self.nth(n) == kind
    }

    /// Consume the next token if `kind` matches, otherwise *do nothing*.
    ///
    /// Return `true` if the token was eaten.
    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.do_bump(kind);
            true
        } else {
            false
        }
    }

    /// Checks if the current token is in `kinds`.
    pub(crate) fn at_ts(&self, kinds: TokenSet) -> bool {
        kinds.contains(self.current())
    }

    /// Checks if the nth token is in `kinds`.
    pub(crate) fn nth_at_ts(&self, n: usize, kinds: TokenSet) -> bool {
        kinds.contains(self.nth(n))
    }

    /// Consume the next token if it is in `kinds` set, otherwise *do nothing*.
    ///
    /// Return `true` if the token was eaten.
    pub(crate) fn eat_ts(&mut self, kinds: TokenSet) -> bool {
        if self.at_ts(kinds) {
            self.bump_any();
            true
        } else {
            false
        }
    }

    /// Starts a new node in the syntax tree. All nodes and tokens consumed
    /// between the `start` and the corresponding `Marker::complete` belong
    /// to the same node.
    pub(crate) fn start(&mut self) -> Marker {
        let pos = self.events.len() as u32;
        self.push_event(Event::tombstone());
        Marker::new(pos)
    }

    /// Consume the next token if it matches `kind`, or panic.
    pub(crate) fn bump(&mut self, kind: SyntaxKind) {
        assert!(self.eat(kind), "expected {}", kind);
    }

    /// Consume the next token if it is in `kinds`, or panic.
    pub(crate) fn bump_ts(&mut self, kinds: TokenSet) {
        assert!(self.eat_ts(kinds), "expected {:?}", kinds.iter().collect::<Vec<_>>());
    }

    /// Advances the parser by one token.
    pub(crate) fn bump_any(&mut self) {
        let kind = self.nth(0);
        if kind == EOF {
            return;
        }
        self.do_bump(kind)
    }

    // TODO: ra does not create error node in case of recovery
    // just push the error event

    /// Create an error node in the syntax tree.
    pub(crate) fn error(&mut self, err: Error) {
        let m = self.start();
        self.push_event(Event::Error { err });
        m.complete(self, ERROR);
    }

    /// Create an error node and bump the next token.
    pub(crate) fn err_and_bump(&mut self, err: Error) {
        let m = self.start();
        self.push_event(Event::Error { err });
        self.bump_any();
        m.complete(self, ERROR);
    }

    /// Create an error node and bump following tokens until a token is seen in the `recovery` set.
    ///
    /// Returns `true` if recovery kicked in.
    pub(crate) fn err_recover(&mut self, err: Error, recovery: TokenSet) -> bool {
        if self.at_ts(recovery) {
            // create an error node only
            self.error(err);
            true
        } else {
            // consume the token as well
            self.err_and_bump(err);
            false
        }
    }

    /// Consume the next token if it matches `kind`, or create an error node otherwise.
    ///
    /// Return `true` if expectation was met.
    pub(crate) fn expect(&mut self, kind: SyntaxKind) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(self.err_with_expected_syntax(kind));
        false
    }

    /// Consume the next token if it matches `kind`, or create an error node otherwise.
    ///
    /// Return `true` if recovery kicked in.
    #[allow(dead_code)]
    pub(crate) fn expect_recover(&mut self, kind: SyntaxKind, recovery: TokenSet) -> bool {
        if self.eat(kind) {
            return false;
        }
        self.err_recover(self.err_with_expected_syntax(kind), recovery)
    }

    /// Consume the next token if it matches `kind`, or create an error node with a
    /// list of expected `kinds` otherwise.
    ///
    /// Return `true` if expectation was met.
    pub(crate) fn expect_with(&mut self, kind: SyntaxKind, kinds: Vec<SyntaxKind>) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error(self.err_with_expected_syntaxes(kinds));
        false
    }

    /// Consume the next token if it is in `kinds`, or create an error node otherwise.
    ///
    /// Return `true` if expectation was met.
    pub(crate) fn expect_ts(&mut self, kinds: TokenSet) -> bool {
        if self.eat_ts(kinds) {
            return true;
        }
        self.error(self.err_with_expected_syntaxes(kinds.iter().collect()));
        false
    }

    /// Consume the next token if it is in `kinds`, or create an error node otherwise.
    ///
    /// Return `true` if expectation was met.
    ///
    /// Unless the token is in the `recovery` set, it will be consumed.
    pub(crate) fn expect_ts_recover(&mut self, kinds: TokenSet, recovery: TokenSet) -> bool {
        if self.expect_ts(kinds) {
            true
        } else {
            if !self.at_ts(recovery) {
                self.bump_any();
            }
            false
        }
    }

    pub(crate) fn err_with_expected_syntax(&self, kind: SyntaxKind) -> Error {
        Error::UnexpectedToken { expected: List::new(vec![kind]), found: self.current() }
    }

    pub(crate) fn err_with_expected_syntaxes(&self, kinds: Vec<SyntaxKind>) -> Error {
        Error::UnexpectedToken { expected: List::new(kinds), found: self.current() }
    }

    fn do_bump(&mut self, kind: SyntaxKind) {
        self.pos += 1;
        self.steps.set(0);
        self.push_event(Event::Token(kind));
    }

    fn push_event(&mut self, event: Event) {
        self.events.push(event)
    }
}

/// See `Parser::start`.
pub(crate) struct Marker {
    pos: u32,
    bomb: DropBomb,
}

impl Marker {
    fn new(pos: u32) -> Marker {
        Marker { pos, bomb: DropBomb::new("Marker must be either completed or abandoned") }
    }

    /// Finishes the syntax tree node and assigns `kind` to it,
    /// and mark the create a `CompletedMarker` for possible future
    /// operation like `.precede()` to deal with forward_parent.
    pub(crate) fn complete(mut self, p: &mut Parser, kind: SyntaxKind) -> CompletedMarker {
        self.bomb.defuse();
        let idx = self.pos as usize;
        match &mut p.events[idx] {
            Event::Start { kind: slot, .. } => {
                *slot = kind;
            }
            _ => unreachable!(),
        }
        // JW: swap the order according to rust-analyzer
        // it makes more sense pushing the event first and then
        // getting the finish position.
        p.push_event(Event::Finish);
        let finish_pos = p.events.len() as u32;
        CompletedMarker::new(self.pos, finish_pos)
    }

    /// Abandons the syntax tree node. All its children
    /// are attached to its parent instead.
    pub(crate) fn abandon(mut self, p: &mut Parser) {
        self.bomb.defuse();
        let idx = self.pos as usize;
        if idx == p.events.len() - 1 {
            assert!(matches!(
                p.events.pop(),
                Some(Event::Start { kind: TOMBSTONE, forward_parent: None })
            ));
        }
    }
}

pub(crate) struct CompletedMarker {
    start_pos: u32,
    finish_pos: u32,
}

impl CompletedMarker {
    fn new(start_pos: u32, finish_pos: u32) -> Self {
        CompletedMarker { start_pos, finish_pos }
    }

    /// This method allows to create a new node which starts
    /// *before* the current one. That is, parser could start
    /// node `A`, then complete it, and then after parsing the
    /// whole `A`, decide that it should have started some node
    /// `B` before starting `A`. `precede` allows to do exactly
    /// that. See also docs about `forward_parent` in `Event::Start`.
    ///
    /// Given completed events `[START, FINISH]` and its corresponding
    /// `CompletedMarker(pos: 0, _)`.
    /// Append a new `START` events as `[START, FINISH, NEWSTART]`,
    /// then mark `NEWSTART` as `START`'s parent with saving its relative
    /// distance to `NEWSTART` into forward_parent(=2 in this case);
    pub(crate) fn precede(self, p: &mut Parser) -> Marker {
        let new_pos = p.start();
        let idx = self.start_pos as usize;
        match &mut p.events[idx] {
            Event::Start { forward_parent, .. } => {
                *forward_parent = Some(new_pos.pos - self.start_pos);
            }
            _ => unreachable!(),
        }
        new_pos
    }

    /// Undo this completion and turns into a `Marker`
    pub(crate) fn undo_completion(self, p: &mut Parser) -> Marker {
        let start_idx = self.start_pos as usize;
        let finish_idx = self.finish_pos as usize;
        match &mut p.events[start_idx] {
            Event::Start { kind, forward_parent: None } => *kind = TOMBSTONE,
            _ => unreachable!(),
        }
        match &mut p.events[finish_idx] {
            slot @ Event::Finish => *slot = Event::tombstone(),
            _ => unreachable!(),
        }
        Marker::new(self.start_pos)
    }
}
