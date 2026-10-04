use crate::{crdt, store::cache::CachedStore};
use std::collections::BTreeSet;

/// Scheduling policy learned for one callee.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CalleeProfile {
    pub conflict_peers: BTreeSet<u64>,
    pub sequential_only: bool,
    pub deferrable: bool,
}

/// Cache-backed store for scheduler records.
pub type CalleeStore = CachedStore<'static, u64, crdt::Bytes>;

impl CalleeStore {
    // fn get(&self, key: &u64) -> CalleeProfile {
    //     self.cache.get(key)
    // }
}
