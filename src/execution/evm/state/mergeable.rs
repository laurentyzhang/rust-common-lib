use alloy_evm::revm::state::EvmState;
use alloy_primitives::map::hash_map::Entry;
use rayon::prelude::*;

use crate::execution::evm::SlotCodec;

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
        let mut accts_to_merge = Vec::with_capacity(self.inner.len().min(other.inner.len()));
        for (address, incoming_account) in other.inner {
            match self.inner.entry(address) {
                Entry::Occupied(entry)
                    if !incoming_account.is_created()
                        && !incoming_account.is_selfdestructed()
                        && !entry.get().is_selfdestructed() =>
                {
                    // Add the both existing and incoming accounts to the list.
                    accts_to_merge.push((address, entry.remove(), incoming_account));
                }
                entry => {
                    // Add new accounts directly to the map.
                    entry.insert_entry(incoming_account);
                }
            }
        }

        let merged_accts: Vec<_> = accts_to_merge
            .into_par_iter()
            .map(|(address, mut existing, mut incoming)| {
                // Preserve creation status when later updates replace account metadata.
                if existing.is_created() {
                    incoming.mark_created();
                }
                // Incoming slots overwrite matching keys, including zero-valued deletions.
                existing.storage.extend(incoming.storage);

                existing.storage.iter_mut().for_each(|(key, slot)| {
                    if SlotCodec::is_api_slot(*key) {
                        // Perform any additional processing for API slots here.
                    }
                }); 
                });

                // Keep the combined storage alongside the incoming account metadata.
                incoming.storage = existing.storage;
                (address, incoming)
            })
            .collect();
        // Update the destination map sequentially after the parallel merges finish.
        self.inner.extend(merged_accts);
    }
}

impl From<MergeableEvmState> for EvmState {
    /// Recover the owned state without copying it.
    fn from(state: MergeableEvmState) -> Self {
        state.inner
    }
}
