use std::ops::Range;
use stdx::packed_option::PackedOption;

use ahash::AHashMap;
use arena::{Arena, IdxRange};
use mir::{Inst, Value};

use super::Derivative;

pub(super) type CacheData = [PackedOption<Value>; 3];

#[derive(Default)]
pub(super) struct BuilderCache {
    pub(super) resolved_derivatives: AHashMap<Option<Derivative>, ResolvedDerivative>,
    pub(super) cache_data: Arena<CacheData>,
    pub(super) derivative_cache: AHashMap<Option<Derivative>, CacheInfo>,
}

impl BuilderCache {
    pub(super) fn clear(&mut self) {
        self.cache_data.clear();
        self.derivative_cache.clear();
        self.resolved_derivatives.clear();
    }
}

#[derive(Debug, PartialEq, Clone, Eq, Hash)]
pub(super) struct ResolvedDerivative {
    pub(super) instrs: Range<Inst>,
    pub(super) cache_instrs: Range<Inst>,
}

impl ResolvedDerivative {
    pub(super) fn root_instr(pos: Inst) -> ResolvedDerivative {
        // The original instruction (so something the user typed) never has a cache and is always the first
        // instruction.
        ResolvedDerivative {
            instrs: pos..Inst::from(u32::from(pos) + 1),
            cache_instrs: pos..Inst::from(u32::from(pos)),
        }
    }

    pub(super) fn instructions(&self) -> impl Iterator<Item = Inst> {
        let instrs: Range<u32> = self.instrs.start.into()..self.instrs.end.into();
        let cache_instrs: Range<u32> = self.cache_instrs.start.into()..self.cache_instrs.end.into();
        cache_instrs.chain(instrs).map(Inst::from)
    }
}

#[derive(Debug, PartialEq, Clone, Eq, Hash)]
pub(super) struct CacheInfo {
    pub(super) insts: Range<Inst>,
    pub(super) data: IdxRange<CacheData>,
}
