use std::fmt::Debug;
use std::iter;
use std::mem;
use stdx::{impl_debug, impl_idx_from};

use ahash::AHashMap;
use bitset::HybridBitSet;
use mir::{FuncRef, KnownDerivatives, Unknown, Value};
use typed_indexmap::TiSet;

/// An opaque reference to a derivative.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct Derivative(u32);
impl_idx_from!(Derivative(u32));
impl_debug! {
    match Derivative {Derivative(i) => "derivative{i}";}
}

impl Derivative {
    pub fn assert_first_order(self) -> Unknown {
        self.0.into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DerivativeInfo {
    pub prev_order: Option<Derivative>,
    pub base: Unknown,
}

pub struct DerivativeIntern<'a> {
    pub unknowns: TiSet<Unknown, Value>,
    pub ddx_calls: &'a AHashMap<FuncRef, (HybridBitSet<Unknown>, HybridBitSet<Unknown>)>,
    pub derivatives: TiSet<Derivative, DerivativeInfo>,
    buf: Vec<Unknown>,
}

impl<'a> DerivativeIntern<'a> {
    pub fn new(known: &'a KnownDerivatives) -> DerivativeIntern<'a> {
        let derivatives = known
            .unknowns
            .iter_enumerated()
            .map(|(base, _)| DerivativeInfo { base, prev_order: None })
            .collect();

        Self {
            unknowns: known.unknowns.clone(),
            ddx_calls: &known.ddx_calls,
            derivatives,
            buf: Vec::with_capacity(8), // don't expect more than 8th order derivative in most code
        }
    }

    pub fn num_derivatives(&self) -> usize {
        self.derivatives.len()
    }

    pub fn num_unknowns(&self) -> usize {
        self.unknowns.len()
    }

    pub fn previous_order(&self, id: Derivative) -> Option<Derivative> {
        self.derivatives[id].prev_order
    }

    pub fn get_unknown(&self, id: Derivative) -> Unknown {
        self.derivatives[id].base
    }

    pub fn get_base_derivative(&self, id: Derivative) -> Derivative {
        self.to_derivative(self.derivatives[id].base)
    }

    pub fn to_derivative(&self, base: Unknown) -> Derivative {
        self.derivatives.unwrap_index(&DerivativeInfo { prev_order: None, base })
    }

    pub fn unknowns(&self, id: Derivative) -> impl Iterator<Item = Unknown> + '_ {
        iter::successors(Some(id), |it| self.previous_order(*it))
            .map(|unknown| self.get_unknown(unknown))
    }

    #[allow(clippy::needless_collect)] // false positive: can't reverse iter::successors
    pub fn unknowns_rev(&self, id: Derivative) -> impl Iterator<Item = Unknown> + '_ {
        let unknowns: Vec<_> = iter::successors(Some(id), |it| self.previous_order(*it))
            .map(|deriv| self.get_unknown(deriv))
            .collect();
        unknowns.into_iter().rev()
    }

    pub fn ensure_unknown(&mut self, val: Value) -> Unknown {
        let unknown = self.unknowns.ensure(val).0;
        self.derivatives.ensure(DerivativeInfo { prev_order: None, base: unknown });
        unknown
    }

    pub fn intern(&mut self, derivative: DerivativeInfo) -> (Derivative, bool) {
        self.derivatives.ensure(derivative)
    }

    pub fn raise_order(&mut self, id: Derivative, next_unknown: Unknown) -> Derivative {
        self.raise_order_with(id, next_unknown, |_| true).unwrap().0
    }

    pub fn raise_order_with(
        &mut self,
        id: Derivative,
        next_unknown: Unknown,
        f: impl Fn(Derivative) -> bool,
    ) -> Option<(Derivative, bool)> {
        // This is safe since we never hand out a reference

        let mut changed = false;
        let mut prev_orders = mem::take(&mut self.buf);
        prev_orders.extend(self.unknowns(id));

        let mut curr = self.to_derivative(next_unknown);
        for base in prev_orders.drain(..).rev() {
            if !f(self.to_derivative(base)) {
                return None;
            }
            let info = DerivativeInfo { prev_order: Some(curr), base };
            (curr, changed) = self.intern(info);
        }

        self.buf = prev_orders;

        Some((curr, changed))
    }
}
