use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::state::EvmState;
use alloy_primitives::map::hash_map::Entry;
use rayon::prelude::*;

use crate::store::traits::Committer;

#[path = "statedb.rs"]
mod statedb;

/// An owned EVM state with ordered parallel merging.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeableEvmState {
    inner: EvmState,
}

impl MergeableEvmState {
    /// Consume an existing EVM state without copying its accounts or storage.
    pub fn new(state: EvmState) -> Self {
        Self { inner: state }
    }

    /// Insert new or replacement accounts, then merge overlapping accounts in parallel.
    /// Incoming values win, including zero-valued slots and account deletion records.
    pub fn merge(&mut self, other: Self) {
        // An empty destination can take the entire incoming map without copying it.
        if self.inner.is_empty() {
            *self = other;
            return;
        }

        // The overlapping are the ones to be updated in parallel. For entires
        // in the incoming map, but not in the destination, they will be inserted
        // directly into the destination map.
        let mut overlapping = Vec::with_capacity(self.inner.len().min(other.inner.len()));
        for (address, incoming) in other.inner {
            match self.inner.entry(address) {
                Entry::Occupied(entry)
                    if !incoming.is_created()
                        && !incoming.is_selfdestructed()
                        && !entry.get().is_selfdestructed() =>
                {
                    // Move both account versions to a worker instead of sharing the map.
                    overlapping.push((address, entry.remove(), incoming));
                }
                entry => {
                    // New accounts and lifecycle replacements use the incoming record directly.
                    entry.insert_entry(incoming);
                }
            }
        }

        let merged: Vec<_> = overlapping
            .into_par_iter()
            .map(|(address, mut existing, mut incoming)| {
                // Preserve creation status when later updates replace account metadata.
                if existing.is_created() {
                    incoming.mark_created();
                }
                // Incoming slots overwrite matching keys, including zero-valued deletions.
                existing.storage.extend(incoming.storage);
                // Keep the combined storage alongside the incoming account metadata.
                incoming.storage = existing.storage;
                (address, incoming)
            })
            .collect();
        // Update the destination map sequentially after the parallel merges finish.
        self.inner.extend(merged);
    }
}

impl From<MergeableEvmState> for EvmState {
    /// Recover the owned state without copying it.
    fn from(state: MergeableEvmState) -> Self {
        state.inner
    }
}

#[derive(Debug, Default)]
pub struct EvmStateCache {
    inner: MergeableEvmState,
    buffered: BTreeMap<u64, MergeableEvmState>,
}

impl EvmStateCache {
    /// Create a cache owning the supplied state and an empty transaction buffer.
    pub fn new(state: EvmState) -> Self {
        Self {
            inner: MergeableEvmState::new(state),
            buffered: BTreeMap::new(),
        }
    }

    /// Consume the cache and return its accumulated state for persistence.
    pub fn into_evm_state(self) -> EvmState {
        self.inner.into()
    }

    /// Merge adjacent buffered states in parallel rounds, preserving transaction-ID order.
    pub fn commit(&mut self) {
        let mut states: Vec<_> = std::mem::take(&mut self.buffered).into_values().collect();

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
            self.inner.merge(state);
        }
    }
}

// id use template
impl Committer<u64, EvmState> for EvmStateCache {
    /// Buffer a transaction state, replacing any earlier state with the same ID.
    fn buffer(&mut self, id: u64, state: EvmState) {
        self.buffered.insert(id, MergeableEvmState::new(state));
    }

    /// Keep only buffered states whose transaction IDs were accepted.
    fn stage(&mut self, accepted: Vec<u64>) {
        let accepted = accepted.into_iter().collect::<HashSet<_>>();
        self.buffered.retain(|id, _| accepted.contains(id));
    }

    /// Commit accepted states in ID order and clear the buffer.
    fn flush(&mut self) {
        self.commit();
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
