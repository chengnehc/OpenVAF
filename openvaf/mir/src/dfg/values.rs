use std::borrow::Borrow;
use stdx::packed_option::PackedOption;

use ahash::AHashMap;
use lasso::Spur;
use typed_index_collections::TiVec;

use super::uses::UseData;
use crate::entities::{Param, Tag};
use crate::{DataFlowGraph, Ieee64, Inst, Use, Value};

#[derive(Clone)]
pub struct DfgValues {
    /// Primary value table with entries for all values
    pub(super) defs: TiVec<Value, ValueData>,

    /// Primary Use table with entries for all uses
    pub(super) uses: TiVec<Use, UseData>,

    /// interned integer constants
    int_consts: AHashMap<i32, Value>,

    /// interned real constants
    real_consts: AHashMap<Ieee64, Value>,

    /// interned string constants
    str_consts: AHashMap<Spur, Value>,
}

#[derive(Clone, Debug)]
pub(super) struct ValueData {
    pub(super) ty: ValueDataType,
    pub(super) uses_head: PackedOption<Use>,
    pub(super) uses_tail: PackedOption<Use>,
    tag: PackedOption<Tag>,
}

impl ValueData {
    pub fn new(ty: ValueDataType, tag: PackedOption<Tag>) -> Self {
        Self { ty, uses_head: None.into(), uses_tail: None.into(), tag }
    }
}

impl From<ValueDataType> for ValueData {
    fn from(ty: ValueDataType) -> Self {
        Self::new(ty, None.into())
    }
}

#[derive(Clone, Debug)]
pub(super) enum ValueDataType {
    Alias(Value),
    Inst { inst: Inst, idx: u16 },
    Param { param: Param },
    Fconst { val: Ieee64 },
    Iconst { val: i32 },
    Sconst { val: Spur },
    True,
    False,
    Invalid,
}

/// Where did a value come from?
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueDef {
    /// Value is the n'th result of an instruction.
    Result(Inst, usize),
    /// Value is the n'th parameter to a function.
    Param(Param),
    /// Value is a predefined constant.
    Const(Const),
    /// Value is invalid.
    Invalid,
}

impl ValueDef {
    #[inline]
    pub fn unwrap_inst(&self) -> Inst {
        self.inst().expect("Value is not an instruction result")
    }
    #[inline]
    pub fn unwrap_result(&self) -> (Inst, usize) {
        self.result().expect("Value is not an instruction result")
    }
    #[inline]
    pub fn unwrap_const(&self) -> Const {
        self.as_const().expect("Value is not a constant")
    }
    #[inline]
    pub fn unwrap_param(&self) -> Param {
        self.as_param().expect("Value is not a parameter")
    }

    /// Get the instruction where the value was defined, if any.
    #[inline]
    pub fn inst(&self) -> Option<Inst> {
        match *self {
            Self::Result(inst, _) => Some(inst),
            _ => None,
        }
    }
    /// Get the instruction and its index where the value was defined, if any.
    #[inline]
    pub fn result(&self) -> Option<(Inst, usize)> {
        match *self {
            Self::Result(inst, i) => Some((inst, i)),
            _ => None,
        }
    }
    /// Use the value defined as a constant.
    #[inline]
    pub fn as_const(&self) -> Option<Const> {
        match *self {
            Self::Const(const_) => Some(const_),
            _ => None,
        }
    }
    /// Use the value defined as a function parameter.
    #[inline]
    pub fn as_param(&self) -> Option<Param> {
        match *self {
            Self::Param(param) => Some(param),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Const {
    Float(Ieee64),
    Int(i32),
    Str(Spur),
    Bool(bool),
}

use consts::*;

/// Predefined constants, with name from `v0` to `v15`.
pub mod consts {
    use super::{DfgValues, Value, ValueDataType};
    use std::f64::consts::{LN_2, LOG10_E};

    /// Allocate and initialize predefined constants.
    macro_rules! consts {
        ( $(pub const $name: ident: $ty: ident = $val: expr;)* ) => {
            consts!(@const 0, $($name),*);
            pub(super) fn init(dst: &mut DfgValues){
                $(consts!(@init $ty dst $val);)*
            }
        };

        (@const $pos: expr, $name: ident $(,$rem: ident)*) => {
            pub const $name: Value = Value::with_number_($pos);
            consts!(@const $pos + 1, $($rem),*);
        };

        (@const $pos: expr,) => {};

        (@init f64 $dst: ident $val: expr) => {
            $dst.fconst($val.into())
        };
        (@init i32 $dst: ident $val: expr) => {
            $dst.iconst($val)
        };
        (@init bool $dst: ident $val: expr) => {
            $dst.make($val, None)
        };
        (@init invalid $dst: ident $val: expr) => {
            $dst.make_invalid()
        };
    }

    // The order of these constant values shall not be changed, otherwise tests would fail.
    consts! {
        // Placeholder for unused values that must remain (in 'phi's)
        pub const GRAVESTONE: invalid = ValueDataType::Invalid;
        pub const FALSE: bool = ValueDataType::False;
        pub const TRUE: bool = ValueDataType::True;
        pub const F_ZERO: f64 = 0.0;
        pub const ZERO: i32 = 0;
        pub const ONE: i32 = 1;
        pub const F_ONE: f64 = 1.0;
        pub const F_N_ONE: f64 = -1.0;
        pub const F_LN2_N: f64 = -LN_2;
        pub const F_LN2: f64 = LN_2;
        pub const F_LOG10_E: f64 = LOG10_E;
        pub const F_TWO: f64 = 2.0;
        pub const N_ONE: i32 = -1;
        pub const F_TEN: f64 = 10.0;
        pub const F_THREE: f64 = 3.0;
        pub const INFINITY: f64 = f64::INFINITY;
    }
}

impl From<bool> for Value {
    fn from(val: bool) -> Self {
        if val {
            TRUE
        } else {
            FALSE
        }
    }
}

impl DfgValues {
    pub fn new() -> Self {
        let mut res = Self {
            defs: TiVec::new(),
            uses: TiVec::new(),
            int_consts: AHashMap::new(),
            real_consts: AHashMap::new(),
            str_consts: AHashMap::new(),
        };
        // initialize predefined constants
        consts::init(&mut res);
        // normalize to plus zero for consts
        res.real_consts.insert((-0f64).into(), F_ZERO);

        res
    }

    pub fn clear(&mut self) {
        self.defs.clear();
        self.uses.clear();
        self.real_consts.clear();
        self.int_consts.clear();
        self.str_consts.clear();
    }

    /// Get the total number of values.
    pub fn num(&self) -> usize {
        self.defs.len()
    }
    /// Check if a value reference is valid.
    pub fn is_valid(&self, v: Value) -> bool {
        usize::from(v) < self.num()
    }
    /// A value is dead if it is not used by any instruction
    pub fn is_dead(&self, v: Value) -> bool {
        self.defs[v].uses_head.is_none()
    }
    /// Get an iterator over all values.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = Value> {
        self.defs.keys()
    }
    /// Get the definition of a value.
    #[inline]
    pub fn def(&self, v: Value) -> ValueDef {
        match self.defs[v].ty {
            ValueDataType::Inst { inst, idx } => ValueDef::Result(inst, idx as usize),
            ValueDataType::Param { param } => ValueDef::Param(param),
            ValueDataType::Fconst { val } => ValueDef::Const(Const::Float(val)),
            ValueDataType::Iconst { val } => ValueDef::Const(Const::Int(val)),
            ValueDataType::Sconst { val } => ValueDef::Const(Const::Str(val)),
            ValueDataType::True => ValueDef::Const(Const::Bool(true)),
            ValueDataType::False => ValueDef::Const(Const::Bool(false)),
            ValueDataType::Alias(_) | ValueDataType::Invalid => ValueDef::Invalid,
        }
    }
    #[inline]
    pub fn def_allow_alias(&self, v: Value) -> ValueDef {
        match self.defs[v].ty {
            ValueDataType::Inst { inst, idx } => ValueDef::Result(inst, idx as usize),
            ValueDataType::Param { param } => ValueDef::Param(param),
            ValueDataType::Fconst { val } => ValueDef::Const(Const::Float(val)),
            ValueDataType::Iconst { val } => ValueDef::Const(Const::Int(val)),
            ValueDataType::Sconst { val } => ValueDef::Const(Const::Str(val)),
            ValueDataType::True => ValueDef::Const(Const::Bool(true)),
            ValueDataType::False => ValueDef::Const(Const::Bool(false)),
            ValueDataType::Alias(alias) => self.def_allow_alias(alias),
            ValueDataType::Invalid => ValueDef::Invalid,
        }
    }

    /// Allocate an extended value entry.
    #[inline]
    pub(super) fn make(&mut self, ty: ValueDataType, tag: Option<Tag>) -> Value {
        let data = ValueData::new(ty, tag.into());
        self.defs.push_and_get_key(data)
    }
    #[inline]
    pub fn make_invalid(&mut self) -> Value {
        self.make(ValueDataType::Invalid, None)
    }
    #[inline]
    pub fn make_param(&mut self, param: Param) -> Value {
        self.make(ValueDataType::Param { param }, None)
    }
    #[inline]
    pub fn make_param_at(&mut self, param: Param, val: Value) {
        self.defs[val].ty = ValueDataType::Param { param };
    }
    #[inline]
    pub fn make_alias(&mut self, val: Value) -> Value {
        self.make(ValueDataType::Alias(val), None)
    }
    #[inline]
    pub fn make_alias_at(&mut self, val: Value, dst: Value) {
        self.defs[dst].ty = ValueDataType::Alias(val);
    }
    /// Note: bool constant (true/false) is never allocated twice.
    pub fn make_const(&mut self, val: Const) -> Value {
        match val {
            Const::Float(val) => self.fconst(val),
            Const::Int(val) => self.iconst(val),
            Const::Str(val) => self.sconst(val),
            Const::Bool(false) => FALSE,
            Const::Bool(true) => TRUE,
        }
    }
    pub fn iconst(&mut self, val: i32) -> Value {
        *self.int_consts.entry(val).or_insert_with(|| {
            let data = ValueDataType::Iconst { val }.into();
            self.defs.push_and_get_key(data)
        })
    }
    pub fn fconst(&mut self, val: Ieee64) -> Value {
        *self.real_consts.entry(val).or_insert_with(|| {
            let data = ValueDataType::Fconst { val }.into();
            self.defs.push_and_get_key(data)
        })
    }
    pub fn sconst(&mut self, val: Spur) -> Value {
        *self.str_consts.entry(val).or_insert_with(|| {
            let data = ValueDataType::Sconst { val }.into();
            self.defs.push_and_get_key(data)
        })
    }
    pub fn iconst_at(&mut self, val: i32, dst: Value) {
        self.int_consts.insert(val, dst);
        self.defs[dst].ty = ValueDataType::Iconst { val };
    }
    pub fn fconst_at(&mut self, val: Ieee64, dst: Value) {
        self.real_consts.insert(val, dst);
        self.defs[dst].ty = ValueDataType::Fconst { val };
    }
    pub fn sconst_at(&mut self, val: Spur, dst: Value) {
        self.str_consts.insert(val, dst);
        self.defs[dst].ty = ValueDataType::Sconst { val };
    }

    pub fn get_tag(&self, val: Value) -> Option<Tag> {
        self.defs[val].tag.expand()
    }
    pub fn set_tag(&mut self, val: Value, tag: Option<Tag>) {
        self.defs[val].tag = tag.into()
    }
}

impl Borrow<DfgValues> for DataFlowGraph {
    fn borrow(&self) -> &DfgValues {
        &self.values
    }
}

impl Default for DfgValues {
    fn default() -> Self {
        Self::new()
    }
}

impl DataFlowGraph {
    #[inline]
    pub fn resolve_alias(&self, v: Value) -> Value {
        let mut val = v;
        while let ValueDataType::Alias(alias) = self.values.defs[val].ty {
            val = alias
        }
        val
    }

    pub fn strip_alias(&mut self) {
        self.strip_alias_after(0);
    }

    pub fn strip_alias_after(&mut self, old_num_vals: usize) {
        let mut stack = Vec::with_capacity(64);
        let vals = old_num_vals..self.num_values();
        for val in vals {
            let mut val: Value = val.into();
            loop {
                let def = unsafe { self.values.defs.get_unchecked(val) };
                let ValueDataType::Alias(alias) = def.ty else { break };
                stack.push(val);
                val = alias
            }
            if let Some(first) = stack.first() {
                self.replace_uses(*first, val);
                for it in stack[1..].iter().copied() {
                    let dst = unsafe { self.values.defs.get_unchecked_mut(it) };
                    dst.ty = ValueDataType::Alias(val);
                }
                stack.clear();
            }
        }
    }
}
