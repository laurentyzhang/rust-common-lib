use alloy_evm::revm::{database_interface::EmptyDB, state::EvmState};

use alloy_evm::revm::state::{EvmStorage, EvmStorageSlot};
use alloy_primitives::{U256, map::hash_map::Entry};
use rayon::prelude::*;

use crate::execution::{ApiAddress, Property, ScalarKeyCodec};

#[derive(Debug)]
pub struct StateDb<DB = EmptyDB> {
    state: EvmState,
    pub(super) fallback: Option<DB>,
}

impl StateDb {
    pub fn new(state: EvmState) -> Self {
        Self {
            state,
            fallback: None,
        }
    }
}

impl Default for StateDb {
    fn default() -> Self {
        Self::new(EvmState::default())
    }
}

impl<DB> StateDb<DB> {
    pub fn with_fallback(state: EvmState, fallback: Option<DB>) -> Self {
        Self { state, fallback }
    }

    pub fn inner(&self) -> &EvmState {
        &self.state
    }

    pub(super) fn inner_mut(&mut self) -> &mut EvmState {
        &mut self.state
    }

    /// Recover the owned EVM state.
    pub fn into_inner(self) -> EvmState {
        self.state
    }

    /// Merge evmState from another `StateDb` into this one, combining account states and storage.
    pub fn merge(&mut self, other: Self) {
        // An empty destination can take the entire incoming map without copying it.
        if self.inner().is_empty() {
            *self.inner_mut() = other.into_inner();
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
                    if !ApiAddress::is_storage_key(key) || !old_acct.storage.contains_key(&key) {
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
        match ScalarKeyCodec::decode(key) {
            Some((_, field, _))
                if field == Property::PosDelta as u8 || field == Property::NegDelta as u8 =>
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
