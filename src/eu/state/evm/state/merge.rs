use alloy_evm::revm::state::{EvmStorage, EvmStorageSlot};
use alloy_primitives::{U256, map::hash_map::Entry};
use rayon::prelude::*;

use super::StateDb;
use crate::eu::{ApiAddress, Field, KeyCodec};

impl StateDb {
    pub fn merge(&mut self, other: Self) {
        // An empty destination can take the entire incoming map without copying it.
        if self.inner().is_empty() {
            *self = other;
            return;
        }

        // Insert new accounts directly and send overlapping accounts to parallel workers.
        let mut accts_to_merge = Vec::with_capacity(self.inner().len().min(other.inner().len()));
        for (address, incoming_account) in other.into_inner() {
            match self.inner_mut().entry(address) {
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
                        Self::handle_api_slot(&mut old_acct.storage, key, new_value);
                    }
                }

                // Keep the combined storage alongside the incoming account metadata.
                new_acct.storage = old_acct.storage;
                (owner_addr, new_acct)
            })
            .collect();
        // Update the destination map sequentially after the parallel merges finish.
        self.inner_mut().extend(merged_accts);
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
