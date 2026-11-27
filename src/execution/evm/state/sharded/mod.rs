//! EVM state split into independently locked shards.

use std::hash::BuildHasher;
use std::sync::RwLock;

use alloy_evm::revm::state::{Account, Bytecode, EvmState, EvmStorageSlot};
use alloy_primitives::map::DefaultHashBuilder;
use alloy_primitives::{Address, B256, U256};


use super::store::{OverlayRead, OverlayWrite};

// A child module, so it reaches the shards directly.
mod lockless;
pub use lockless::{Batch, LocklessWriter};
#[cfg_attr(not(feature = "mdbx"), allow(unused_imports))]
pub(crate) use lockless::update;

/// Returns which of `N` shards holds `address`'s account info.
fn account_shard<const N: usize>(hasher: &DefaultHashBuilder, address: Address) -> usize {
    hasher.hash_one(address) as usize % N
}

/// Returns which of `N` shards holds `address`'s slot `key`.
fn slot_shard<const N: usize>(hasher: &DefaultHashBuilder, address: Address, key: U256) -> usize {
    hasher.hash_one((address, key)) as usize % N
}

/// EVM state split into `N` shards, each behind its own lock. An account's info lives in the
/// shard of its address; each slot lives in the shard of its (address, key), so one account may
/// appear in several shards, each holding part of its storage. Access through `&self` locks the
/// shard; access through `&mut self` needs no lock.
pub struct Sharded<const N: usize = 64> {
    shards: [RwLock<EvmState>; N],
    hasher: DefaultHashBuilder,
}

impl<const N: usize> Sharded<N> {
    /// Creates `N` empty shards.
    pub fn new() -> Self {
        Self {
            shards: std::array::from_fn(|_| RwLock::default()),
            hasher: DefaultHashBuilder::default(),
        }
    }

    /// Returns the index of the shard holding `address`'s account info.
    fn index(&self, address: Address) -> usize {
        account_shard::<N>(&self.hasher, address)
    }

    /// Returns the index of the shard holding `address`'s slot `key`.
    fn slot_index(&self, address: Address, key: U256) -> usize {
        slot_shard::<N>(&self.hasher, address, key)
    }

    /// Reads `address`'s account info under its shard's read lock; its storage there is partial.
    pub fn read<R>(&self, address: Address, f: impl FnOnce(Option<&Account>) -> R) -> R {
        f(self.shards[self.index(address)].read().unwrap().get(&address))
    }

    /// Updates `address`'s account info under its shard's write lock, inserting a default if
    /// absent; slots go through `write_slot`.
    pub fn write<R>(&self, address: Address, f: impl FnOnce(&mut Account) -> R) -> R {
        f(self.shards[self.index(address)]
            .write()
            .unwrap()
            .entry(address)
            .or_default())
    }

    /// Reads `address`'s slot `key` under its shard's read lock.
    pub fn read_slot<R>(
        &self,
        address: Address,
        key: U256,
        f: impl FnOnce(Option<&EvmStorageSlot>) -> R,
    ) -> R {
        let shard = self.shards[self.slot_index(address, key)].read().unwrap();
        f(shard.get(&address).and_then(|account| account.storage.get(&key)))
    }

    /// Updates `address`'s slot `key` under its shard's write lock, inserting a default if absent.
    pub fn write_slot<R>(
        &self,
        address: Address,
        key: U256,
        f: impl FnOnce(&mut EvmStorageSlot) -> R,
    ) -> R {
        let mut shard = self.shards[self.slot_index(address, key)].write().unwrap();
        f(shard.entry(address).or_default().storage.entry(key).or_default())
    }

    /// Inserts the account into its address's shard unless an entry is already there.
    pub(crate) fn cache_account(&self, address: Address, account: Account) {
        self.shards[self.index(address)]
            .write()
            .unwrap()
            .entry(address)
            .or_insert(account);
    }

    /// Inserts the slot into its (address, key) shard unless it is already there. In the
    /// account's own shard it is skipped while the account has no entry, so a cached slot never
    /// stands in for the account.
    pub(crate) fn cache_slot(&self, address: Address, key: U256, slot: EvmStorageSlot) {
        let index = self.slot_index(address, key);
        let mut shard = self.shards[index].write().unwrap();
        if index == self.index(address) && !shard.contains_key(&address) {
            return;
        }
        shard
            .entry(address)
            .or_default()
            .storage
            .entry(key)
            .or_insert(slot);
    }

    /// Merges all shards into one `EvmState`: each account takes its info from its address's
    /// shard and its storage from every shard.
    pub fn into_inner(self) -> EvmState {
        let mut state = EvmState::default();
        let mut parts = Vec::new();
        for (index, shard) in self.shards.into_iter().enumerate() {
            for (address, account) in shard.into_inner().unwrap() {
                if account_shard::<N>(&self.hasher, address) == index {
                    state.insert(address, account);
                } else {
                    parts.push((address, account.storage));
                }
            }
        }
        for (address, storage) in parts {
            state.entry(address).or_default().storage.extend(storage);
        }
        state
    }
}

impl<const N: usize> std::fmt::Debug for Sharded<N> {
    /// Shows the shards; the hasher has no `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sharded")
            .field("shards", &self.shards)
            .finish_non_exhaustive()
    }
}

impl<const N: usize> OverlayRead for Sharded<N> {
    /// Reads the account info from its address's shard.
    fn account<R>(&self, address: Address, f: impl FnOnce(Option<&Account>) -> R) -> R {
        self.read(address, f)
    }

    /// Reads the slot from its (address, key) shard.
    fn slot<R>(
        &self,
        address: Address,
        key: U256,
        f: impl FnOnce(Option<&EvmStorageSlot>) -> R,
    ) -> R {
        self.read_slot(address, key, f)
    }

    /// Searches every shard for the code.
    fn code(&self, code_hash: B256) -> Option<Bytecode> {
        self.shards.iter().find_map(|shard| {
            shard
                .read()
                .unwrap()
                .values()
                .filter(|account| account.info.code_hash == code_hash)
                .find_map(|account| account.info.code.clone())
        })
    }
}



impl<const N: usize> OverlayWrite for Sharded<N> {
    /// Updates the account info in its address's shard, inserting `init()` if absent; no lock.
    fn account_mut<R>(
        &mut self,
        address: Address,
        init: impl FnOnce() -> Account,
        f: impl FnOnce(&mut Account) -> R,
    ) -> R {
        let index = self.index(address);
        f(self.shards[index]
            .get_mut()
            .unwrap()
            .entry(address)
            .or_insert_with(init))
    }

    /// Updates the slot in its (address, key) shard, inserting `init()` if absent; no lock.
    fn slot_mut<R>(
        &mut self,
        address: Address,
        key: U256,
        init: impl FnOnce() -> EvmStorageSlot,
        f: impl FnOnce(&mut EvmStorageSlot) -> R,
    ) -> R {
        let index = self.slot_index(address, key);
        f(self.shards[index]
            .get_mut()
            .unwrap()
            .entry(address)
            .or_default()
            .storage
            .entry(key)
            .or_insert_with(init))
    }

    /// Merges all shards into one `EvmState`.
    fn into_evm_state(self) -> EvmState {
        self.into_inner()
    }

    /// Routes the accounts to their shards, then writes every shard in parallel; no lock.
    fn write_accounts(&mut self, accounts: EvmState) {
        let batches = self.partition(accounts);
        self.write_batches(batches);
    }

    type View<'a> = lockless::View<'a, N>;

    /// Views every shard through `&mut`, without locks.
    fn view(&mut self) -> lockless::View<'_, N> {
        lockless::View {
            shards: self.shards.each_mut().map(|shard| &*shard.get_mut().unwrap()),
            hasher: &self.hasher,
        }
    }
}

#[cfg(test)]
#[path = "tests/sharded.rs"]
mod tests;
