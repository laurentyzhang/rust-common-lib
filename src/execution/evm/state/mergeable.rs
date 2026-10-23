use alloy_evm::revm::state::EvmState;
use alloy_primitives::map::hash_map::Entry;
use rayon::prelude::*;

/// An owned EVM state with ordered parallel merging.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeableEvmState {
    pub inner: EvmState,
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

        // Insert new accounts directly and send overlapping accounts to parallel workers.
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
