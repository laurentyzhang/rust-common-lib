use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::state::EvmState;
use rayon::prelude::*;

use super::super::{AccountStorage, StateDb};
use crate::store::traits::Committer;

/// Buffers transaction states and merges accepted changes into an EVM state map.
#[derive(Debug, Default)]
pub struct StatDbCommitter {
    state: StateDb,
    buffered: BTreeMap<u64, StateDb>,
}

impl StatDbCommitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn merge(&mut self, other: Self) {
        // An empty destination can take the entire incoming map without copying it.
        if self.state.is_empty() {
            *self = other;
            return;
        }

        // Insert new accounts directly and send overlapping accounts to parallel workers.
        let mut accts_to_merge = Vec::with_capacity(self.state.len().min(other.state.len()));
        for (address, incoming_account) in other.state {
            match self.state.entry(address) {
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
            .map(|(owner_addr, mut old_acct, mut new_acct)| {
                // Preserve creation status when later updates replace account metadata.
                if old_acct.is_created() {
                    new_acct.mark_created();
                }

                for (key, new_value) in new_acct.storage {
                    if !KeyCodec::is_api_key(key) || !old_acct.storage.contains_key(&key) {
                        old_acct.storage.insert(key, new_value);
                    } else {
                        handle_api_slot(&mut old_acct.storage, key, new_value);
                    }
                }

                // Keep the combined storage alongside the incoming account metadata.
                new_acct.storage = old_acct.storage;
                (owner_addr, new_acct)
            })
            .collect();
        // Update the destination map sequentially after the parallel merges finish.
        self.state.extend(merged_accts);
    }

    /// Dispatches CRDT delta merging by API type.
    fn handle_api_slot(storage: &mut EvmStorage, key: U256, new_value: EvmStorageSlot) {
        match KeyCodec::decode(key) {
            Some((ApiAddress::Scalar, field, _))
                if field == Field::PosDelta as u8 || field == Field::NegDelta as u8 =>
            {
                // Apply only this state's change to the delta magnitude.
                let change = new_value
                    .present_value()
                    .wrapping_sub(new_value.original_value());
                let slot = storage
                    .get_mut(&key)
                    .expect("API slot exists before handling");
                slot.present_value = slot.present_value().wrapping_add(change);
            }
            _ => {}
        }
    }
}

impl Committer<u64, EvmState> for StatDbCommitter {
    /// Buffer a transaction state, replacing any earlier state with the same ID.
    fn buffer(&mut self, id: u64, state: EvmState) {
        self.buffered.insert(id, StateDb::new(state));
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
            self.state.merge(state);
        }
    }
}
