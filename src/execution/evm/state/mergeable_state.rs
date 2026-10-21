use crate::store::traits::Committer;
use std::{
    collections::{BTreeMap, HashSet},
    convert::Infallible,
};

use alloy_evm::revm::{
    Database,
    state::{AccountInfo, Bytecode, EvmState},
};
use alloy_primitives::{Address, B256, U256};
use rayon::prelude::*;

/// An owned EVM state with parallel merging and REVM database access.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MergeableEvmState {
    inner: EvmState,
    // Preserve storage resets when later updates replace account status flags.
    storage_resets: HashSet<Address>,
}

impl MergeableEvmState {
    /// Consume an existing EVM state without copying its accounts or storage.
    pub fn new(state: EvmState) -> Self {
        let storage_resets = state
            .iter()
            .filter(|(_, account)| account.is_created() || account.is_selfdestructed())
            .map(|(address, _)| *address)
            .collect();
        Self {
            inner: state,
            storage_resets,
        }
    }

    /// Move new accounts and slots sequentially, then merge existing accounts in parallel.
    /// Incoming metadata and overlapping slots win; creation or deletion resets prior storage.
    pub fn merge(&mut self, mut other: Self) {
        if self.inner.is_empty() {
            *self = other;
            return;
        }

        other.inner.retain(|address, incoming| {
            let Some(account) = self.inner.get_mut(address) else {
                self.inner.insert(*address, std::mem::take(incoming));
                return false;
            };
            if other.storage_resets.contains(address) {
                account.storage.clear();
            }

            incoming.storage.retain(|key, slot| {
                if account.storage.contains_key(key) {
                    return true;
                }
                account.storage.insert(*key, std::mem::take(slot));
                false
            });
            true
        });

        let mut updates = Vec::with_capacity(other.inner.len());
        updates.extend(self.inner.iter_mut().filter_map(|(address, account)| {
            other
                .inner
                .remove(address)
                .map(|incoming| (account, incoming))
        }));

        updates.into_par_iter().for_each(|(account, mut incoming)| {
            for (key, value) in incoming.storage.drain() {
                *account
                    .storage
                    .get_mut(&key)
                    .expect("slot exists after preparation") = value;
            }

            // Move incoming metadata while retaining the prepared storage map.
            std::mem::swap(&mut account.storage, &mut incoming.storage);
            *account = incoming;
        });
        self.storage_resets.extend(other.storage_resets);
    }
}

impl From<MergeableEvmState> for EvmState {
    /// Recover the owned state without copying it.
    fn from(state: MergeableEvmState) -> Self {
        state.inner
    }
}

#[derive(Default)]
pub struct EvmStateCache {
    inner: MergeableEvmState,
    committer: EvmStateCacheCommitter,
}

impl Database for EvmStateCache {
    type Error = Infallible;

    /// Return account information unless the account is absent or deleted.
    fn basic(&mut self, address: Address) -> Result<Option<AccountInfo>, Self::Error> {
        Ok(self.inner.inner.get(&address).and_then(|account| {
            if account.is_selfdestructed()
                || (account.is_loaded_as_not_existing() && !account.is_created())
            {
                return None;
            }
            Some(account.info.clone())
        }))
    }

    /// Find embedded bytecode by hash, returning empty code when unavailable.
    fn code_by_hash(&mut self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self
            .inner
            .inner
            .values()
            .filter(|account| account.info.code_hash == code_hash)
            .find_map(|account| account.info.code.clone())
            .unwrap_or_default())
    }

    /// Return a slot's present value, or zero for absent accounts and slots.
    fn storage(&mut self, address: Address, index: U256) -> Result<U256, Self::Error> {
        let Some(account) = self.inner.inner.get(&address) else {
            return Ok(U256::ZERO);
        };
        if account.is_selfdestructed()
            || (account.is_loaded_as_not_existing() && !account.is_created())
        {
            return Ok(U256::ZERO);
        }
        Ok(account
            .storage
            .get(&index)
            .map(|slot| slot.present_value())
            .unwrap_or_default())
    }

    /// EVM state contains no block history, so block hashes default to zero.
    fn block_hash(&mut self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

#[derive(Default)]
struct EvmStateCacheCommitter {
    buffered: BTreeMap<u64, MergeableEvmState>,
}

impl EvmStateCache {
    /// Create a cache owning the supplied state and an empty transaction buffer.
    pub fn new(state: EvmState) -> Self {
        Self {
            inner: MergeableEvmState::new(state),
            committer: EvmStateCacheCommitter::default(),
        }
    }

    /// Merge adjacent buffered states in parallel rounds, preserving transaction-ID order.
    pub fn commit(&mut self) {
        let mut states: Vec<_> = std::mem::take(&mut self.committer.buffered)
            .into_values()
            .collect();

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

impl Committer<EvmState> for EvmStateCache {
    /// Buffer a transaction state, replacing any earlier state with the same ID.
    fn buffer(&mut self, id: u64, state: EvmState) {
        self.committer
            .buffered
            .insert(id, MergeableEvmState::new(state));
    }

    /// Keep only buffered states whose transaction IDs were accepted.
    fn stage(&mut self, accepted: Vec<u64>) {
        let accepted = accepted.into_iter().collect::<HashSet<_>>();
        self.committer
            .buffered
            .retain(|id, _| accepted.contains(id));
    }

    /// Commit accepted states in ID order and clear the buffer.
    fn flush(&mut self) {
        self.commit();
    }
}

#[cfg(test)]
mod tests;
