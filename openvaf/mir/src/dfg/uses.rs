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

impl DfgValues {
    pub fn use_to_user(&self, use_: Use) -> Inst {
        self.uses[use_].user
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

    pub fn uses(&self, value: Value) -> UseIter {
        let cursor = self.uses_cursor(value);
        UseIter { cursor, dfg: self }
    }

    fn uses_cursor(&self, value: Value) -> UseCursor {
        UseCursor { head: self.defs[value].uses_head, tail: self.defs[value].uses_tail }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub struct UseCursor {
    head: PackedOption<Use>,
    tail: PackedOption<Use>,
}

impl UseCursor {
    pub fn advance(&mut self, values: &DfgValues) -> Option<Use> {
        let use_ = self.head.expand()?;
        if self.head == self.tail {
            self.head = None.into();
            self.tail = None.into();
        } else {
            self.head = values.uses[use_].next;
        }
        Some(use_)
    }

    pub fn advance_back(&mut self, values: &DfgValues) -> Option<Use> {
        let use_ = self.tail.expand()?;
        if self.head == self.tail {
            self.head = None.into();
            self.tail = None.into();
        } else {
            self.tail = values.uses[use_].prev;
        }
        Some(use_)
    }
}

#[derive(Clone)]
pub struct UseIter<'a> {
    dfg: &'a DfgValues,
    cursor: UseCursor,
}

impl Iterator for UseIter<'_> {
    type Item = Use;

    fn next(&mut self) -> Option<Self::Item> {
        self.cursor.advance(self.dfg)
    }
}

impl DoubleEndedIterator for UseIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.cursor.advance_back(self.dfg)
    }
}

impl DfgValues {
    /// Create a new use of `val` as the `idx` argument of `inst`
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

    /// Attach a use to a value.
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
    /// Change all uses of `dst` to behave as if they used value `src`.
    /// The `dst` value can't be attached to an instruction or block then.
    ///
    /// # Note
    /// Calling this value with `dst` == `src` will cause incorrect results
    pub fn replace_uses(&mut self, dst: Value, src: Value) {
        debug_assert_ne!(dst, src);

        if self.values.get_tag(src).is_none() {
            self.values.set_tag(src, self.values.get_tag(dst))
        }

        // replace values in instructions
        let mut cursor = self.values.uses_cursor(dst);
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
        let cursor = vals.next().map(|v| self.values.uses_cursor(*v)).unwrap_or_default();
        InstUseIter { cursor, vals, dfg: &self.values }
    }

    // pub fn use_set_value(&mut self, use_: Use, val: Value) {
    //     debug_assert!(!self.is_use_detached(use_));
    //     self.values.detach_use(use_, &self.insts);
    //     let data = self.values.uses[use_];
    //     self.insts.args_mut(data.parent)[data.parent_idx as usize] = val;
    //     self.attach_use(use_, val);
    // }
}

#[derive(Clone)]
pub struct InstUseIter<'a> {
    dfg: &'a DfgValues,
    vals: slice::Iter<'a, Value>,
    cursor: UseCursor,
}

impl Iterator for InstUseIter<'_> {
    type Item = Use;

    fn next(&mut self) -> Option<Self::Item> {
        let mut use_ = self.cursor.advance(self.dfg);
        if use_.is_none() {
            for val in &mut self.vals {
                self.cursor = self.dfg.uses_cursor(*val);
                let new_use = self.cursor.advance(self.dfg)?;
                use_ = Some(new_use);
            }
        }
        use_
    }
}
