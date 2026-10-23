use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::state::EvmState;
use rayon::prelude::*;

use super::{EvmStateCache, MergeableEvmState};
use crate::store::traits::Committer;

/// Buffers transaction states and merges accepted changes into an EVM state cache.
#[derive(Debug, Default)]
pub struct EvmStateCommitter {
    cache: EvmStateCache,
    buffered: BTreeMap<u64, MergeableEvmState>,
}

impl EvmStateCommitter {
    pub fn new(cache: EvmStateCache) -> Self {
        Self {
            cache,
            buffered: BTreeMap::new(),
        }
    }

    /// Recover the cache; unflushed transaction states are discarded.
    pub fn into_inner(self) -> EvmStateCache {
        self.cache
    }
}

impl Committer<u64, EvmState> for EvmStateCommitter {
    /// Buffer a transaction state, replacing any earlier state with the same ID.
    fn buffer(&mut self, id: u64, state: EvmState) {
        self.buffered.insert(id, MergeableEvmState::new(state));
    }

    /// Keep only buffered states whose transaction IDs were accepted.
    fn stage(&mut self, accepted: Vec<u64>) {
        let accepted = accepted.into_iter().collect::<HashSet<_>>();
        self.buffered.retain(|id, _| accepted.contains(id));
    }

    /// Merge buffered states in transaction-ID order and clear the buffer.
    fn flush(&mut self) {
        let mut states: Vec<_> = std::mem::take(&mut self.buffered).into_values().collect();

        // Merge adjacent states in parallel rounds without changing their order.
        while states.len() > 1 {
            states.par_chunks_mut(2).for_each(|pair| {
                if let [left, right] = pair {
                    left.merge(std::mem::take(right));
                }
            });

            // Keep each left result and carry an unpaired last state to the next round.
            let remaining = states.len().div_ceil(2);
            for index in 1..remaining {
                states.swap(index, index * 2);
            }
            states.truncate(remaining);
        }

        if let Some(state) = states.pop() {
            self.cache.inner.merge(state);
        }
    }
}
