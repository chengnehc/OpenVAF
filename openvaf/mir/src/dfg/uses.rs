use std::borrow::Borrow;
use std::{mem, slice};
use stdx::packed_option::PackedOption;

use crate::{DataFlowGraph, Inst, Use, Value};

use super::insts::DfgInsts;
use super::values::DfgValues;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct UseData {
    pub(super) user: Inst,
    pub(super) idx: u16,
    attached: bool,
    prev: PackedOption<Use>,
    next: PackedOption<Use>,
}

impl Use {
    pub fn prev(self, values: &DfgValues) -> Option<Use> {
        values.uses[self].prev.expand()
    }

    pub fn next(self, values: &DfgValues) -> Option<Use> {
        values.uses[self].next.expand()
    }

    pub fn to_user(self, dfg: &DataFlowGraph) -> Inst {
        dfg.use_to_user(self)
    }

    pub fn to_value(self, dfg: &DataFlowGraph) -> Value {
        dfg.use_to_value(self)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct UseCursor {
    pub curr: Option<Use>,
}

impl UseCursor {
    /// returns the current use and advances the cursor forward
    pub fn advance(&mut self, dfg: &DfgValues) -> Option<Use> {
        let res = self.curr?;
        self.curr = res.next(dfg);
        Some(res)
    }

    /// returns the current use and advances the cursor backward
    pub fn advance_back(&mut self, dfg: &DfgValues) -> Option<Use> {
        let res = self.curr?;
        self.curr = res.prev(dfg);
        Some(res)
    }

    pub fn into_iter<D: Borrow<DfgValues>>(self, dfg: &D) -> UseIter<'_> {
        UseIter { cursor: self, dfg: dfg.borrow() }
    }
}

impl From<Option<Use>> for UseCursor {
    fn from(curr: Option<Use>) -> Self {
        UseCursor { curr }
    }
}

impl From<PackedOption<Use>> for UseCursor {
    fn from(curr: PackedOption<Use>) -> Self {
        UseCursor { curr: curr.expand() }
    }
}

#[derive(Clone)]
pub struct UseIter<'a> {
    pub cursor: UseCursor,
    dfg: &'a DfgValues,
}

impl Iterator for UseIter<'_> {
    type Item = Use;

    fn next(&mut self) -> Option<Self::Item> {
        self.cursor.advance(self.dfg)
    }
}

#[derive(Clone)]
pub struct InstUseIter<'a> {
    vals: slice::Iter<'a, Value>,
    cursor: UseCursor,
    dfg: &'a DfgValues,
}

impl Iterator for InstUseIter<'_> {
    type Item = Use;

    fn next(&mut self) -> Option<Self::Item> {
        let mut res = self.cursor.advance(self.dfg);
        if res.is_none() {
            for val in &mut self.vals {
                self.cursor = self.dfg.uses_head_cursor(*val);
                if let Some(new_res) = self.cursor.advance(self.dfg) {
                    res = Some(new_res);
                    break;
                }
            }
        }
        res
    }
}

#[derive(Copy, Clone)]
pub struct DoubleEndedUseIter<'a> {
    head: Option<Use>,
    tail: Option<Use>,
    dfg: &'a DfgValues,
}

impl Iterator for DoubleEndedUseIter<'_> {
    type Item = Use;

    fn next(&mut self) -> Option<Self::Item> {
        let rval = self.head;
        if let Some(use_) = rval {
            if self.head == self.tail {
                self.head = None;
                self.tail = None;
            } else {
                self.head = use_.next(self.dfg);
            }
        }
        rval
    }
}

impl DoubleEndedIterator for DoubleEndedUseIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        let rval = self.tail;
        if let Some(use_) = rval {
            if self.head == self.tail {
                self.head = None;
                self.tail = None;
            } else {
                self.tail = use_.prev(self.dfg);
            }
        }
        rval
    }
}

impl DfgValues {
    pub fn uses(&self, value: Value) -> UseIter {
        self.uses_head_cursor(value).into_iter(self)
    }

    pub fn uses_double_ended(&self, value: Value) -> DoubleEndedUseIter<'_> {
        DoubleEndedUseIter {
            head: self.defs[value].uses_head.expand(),
            tail: self.defs[value].uses_tail.expand(),
            dfg: self,
        }
    }

    fn uses_head_cursor(&self, value: Value) -> UseCursor {
        self.defs[value].uses_head.into()
    }

    #[allow(unused)]
    fn uses_tail_cursor(&self, value: Value) -> UseCursor {
        self.defs[value].uses_tail.into()
    }

    pub fn use_to_user(&self, use_: Use) -> Inst {
        self.uses[use_].user
    }

    pub fn use_to_operand(&self, use_: Use) -> (Inst, u16) {
        (self.uses[use_].user, self.uses[use_].idx)
    }

    pub fn use_to_value(&self, use_: Use, insts: &DfgInsts) -> Value {
        let UseData { user, idx, .. } = self.uses[use_];
        insts.args(user)[idx as usize]
    }

    pub fn use_to_value_mut<'a>(&self, use_: Use, insts: &'a mut DfgInsts) -> &'a mut Value {
        let UseData { user, idx, .. } = self.uses[use_];
        &mut insts.args_mut(user)[idx as usize]
    }

    pub fn is_use_detached(&self, use_: Use) -> bool {
        !self.uses[use_].attached
    }

    pub fn make_use(&mut self, val: Value, user: Inst, idx: u16) -> Use {
        let def = &mut self.defs[val];
        let use_ = self.uses.push_and_get_key(UseData {
            user,
            idx,
            attached: true,
            prev: None.into(),
            next: def.uses_head,
        });
        if let Some(old_head) = def.uses_head.expand() {
            self.uses[old_head].prev = use_.into();
        } else {
            def.uses_tail = use_.into();
        }
        def.uses_head = use_.into();

        use_
    }

    pub fn attach_use(&mut self, use_: Use, val: Value) {
        debug_assert!(
            self.is_use_detached(use_),
            "use_ must be detached from old value before being added back"
        );
        let data = &mut self.uses[use_];
        data.attached = true;
        if let Some(old_head) = self.defs[val].uses_head.expand() {
            data.next = old_head.into();
            self.uses[old_head].prev = use_.into();
        } else {
            self.defs[val].uses_tail = use_.into();
        }
        self.defs[val].uses_head = use_.into();
    }

    pub fn detach_use(&mut self, use_: Use, insts: &DfgInsts) {
        let prev = mem::take(&mut self.uses[use_].prev);
        let next = mem::take(&mut self.uses[use_].next);
        if !mem::take(&mut self.uses[use_].attached) {
            return; // already detached
        }
        match (next.expand(), prev.expand()) {
            (Some(next_), Some(prev_)) => {
                self.uses[next_].prev = prev;
                self.uses[prev_].next = next;
            }
            (None, None) => {
                let val = self.use_to_value(use_, insts);
                self.defs[val].uses_head = None.into();
                self.defs[val].uses_tail = None.into();
            }
            (Some(next_), None) => {
                let val = self.use_to_value(use_, insts);
                self.defs[val].uses_head = next_.into();
                self.uses[next_].prev = None.into();
            }
            (None, Some(prev_)) => {
                let val = self.use_to_value(use_, insts);
                self.defs[val].uses_tail = prev_.into();
                self.uses[prev_].next = None.into();
            }
        }
    }
}

impl DataFlowGraph {
    /// Turn a value into an alias of another.
    ///
    /// Change the `dst` value to behave as an alias of `src`. This means that all uses of `dst`
    /// will behave as if they used that value `src`.
    ///
    /// The `dst` value can't be attached to an instruction or block.
    ///
    /// # Note
    /// Calling this value with `dst` == `src` will cause incorrect results
    pub fn replace_uses(&mut self, dst: Value, src: Value) {
        debug_assert_ne!(dst, src);

        if self.values.get_tag(src).is_none() {
            self.values.set_tag(src, self.values.get_tag(dst))
        }

        // replace values in instructions
        let mut cursor = self.values.uses_head_cursor(dst);
        while let Some(use_) = cursor.advance(&self.values) {
            *self.use_to_value_mut(use_) = src;
        }

        // Update use list
        if let Some(new_head) = self.values.defs[dst].uses_head.take() {
            if let Some(old_head) = self.values.defs[src].uses_head.expand() {
                let old_tail = self.values.defs[dst].uses_tail.unwrap();
                self.values.uses[old_tail].next = old_head.into();
                self.values.uses[old_head].prev = old_tail.into();
            } else {
                self.values.defs[src].uses_tail = self.values.defs[dst].uses_tail;
            }
            self.values.defs[dst].uses_tail = None.into();
            self.values.defs[src].uses_head = new_head.into();
        }
    }

    pub fn inst_uses(&self, inst: Inst) -> InstUseIter {
        let mut vals = self.inst_results(inst).iter();
        let cursor = vals.next().map(|res| self.values.uses_head_cursor(*res)).unwrap_or_default();
        InstUseIter { cursor, vals, dfg: &self.values }
    }
}
