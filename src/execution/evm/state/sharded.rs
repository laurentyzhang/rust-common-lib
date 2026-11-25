//! EVM state split into shards: plain shards for one user, locked shards for many.

use std::hash::BuildHasher;
use std::sync::RwLock;

use alloy_evm::revm::state::{Account, Bytecode, EvmState, EvmStorageSlot};
use alloy_primitives::map::DefaultHashBuilder;
use alloy_primitives::{Address, B256, U256};

use super::overlay::OverlayState;

/// EVM state split into `N` shards. An account's info lives in the shard of its address; each
/// slot lives in the shard of its (address, key), so one account may appear in several shards,
/// each holding part of its storage.
pub struct Sharded<Shard, const N: usize = 64> {
    shards: [Shard; N],
    hasher: DefaultHashBuilder,
}

/// Shards behind one lock each, for concurrent users.
pub type Shared<const N: usize = 64> = Sharded<RwLock<EvmState>, N>;

impl<Shard: Default, const N: usize> Sharded<Shard, N> {
    /// Creates `N` empty shards.
    pub fn new() -> Self {
        Self {
            shards: std::array::from_fn(|_| Shard::default()),
            hasher: DefaultHashBuilder::default(),
        }
    }

    /// Returns the index of the shard holding `address`'s account info.
    fn index(&self, address: Address) -> usize {
        self.hasher.hash_one(address) as usize % N
    }

    /// Returns the index of the shard holding `address`'s slot `key`.
    fn slot_index(&self, address: Address, key: U256) -> usize {
        self.hasher.hash_one((address, key)) as usize % N
    }
}

/// Lock-free access: the caller holds `&` or `&mut`.
impl<const N: usize> Sharded<EvmState, N> {
    /// Returns `address`'s account info; its storage there is partial.
    pub fn account(&self, address: Address) -> Option<&Account> {
        self.shards[self.index(address)].get(&address)
    }

    /// Returns `address`'s account info, inserting a default if absent; slots go through
    /// `slot_mut`.
    pub fn account_mut(&mut self, address: Address) -> &mut Account {
        let index = self.index(address);
        self.shards[index].entry(address).or_default()
    }

    /// Returns `address`'s slot `key`.
    pub fn slot(&self, address: Address, key: U256) -> Option<&EvmStorageSlot> {
        self.shards[self.slot_index(address, key)]
            .get(&address)
            .and_then(|account| account.storage.get(&key))
    }

    /// Returns `address`'s slot `key`, inserting a default if absent.
    pub fn slot_mut(&mut self, address: Address, key: U256) -> &mut EvmStorageSlot {
        let index = self.slot_index(address, key);
        self.shards[index]
            .entry(address)
            .or_default()
            .storage
            .entry(key)
            .or_default()
    }

    /// Merges all shards into one `EvmState`: each account takes its info from its address's
    /// shard and its storage from every shard.
    pub fn into_inner(self) -> EvmState {
        let Self { shards, hasher } = self;
        let home = |address: Address| hasher.hash_one(address) as usize % N;
        let mut state = EvmState::default();
        let mut parts = Vec::new();
        for (index, shard) in shards.into_iter().enumerate() {
            for (address, account) in shard {
                if home(address) == index {
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

/// Concurrent access: each call locks only its shard.
impl<const N: usize> Shared<N> {
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

    /// Removes the locks, leaving the lock-free form; nothing is copied.
    pub fn into_unlocked(self) -> Sharded<EvmState, N> {
        Sharded {
            shards: self.shards.map(|shard| shard.into_inner().unwrap()),
            hasher: self.hasher,
        }
    }
}

impl<Shard: std::fmt::Debug, const N: usize> std::fmt::Debug for Sharded<Shard, N> {
    /// Shows the shards; the hasher has no `Debug`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sharded")
            .field("shards", &self.shards)
            .finish_non_exhaustive()
    }
}

impl<const N: usize> OverlayState for Shared<N> {
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

    /// Updates the account info in its address's shard, inserting `init()` if absent; `&mut`
    /// access needs no lock.
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

    /// Updates the slot in its (address, key) shard, inserting `init()` if absent; `&mut` access
    /// needs no lock.
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

    /// Inserts the account into its address's shard unless an entry is already there.
    fn cache_account(&self, address: Address, account: Account) {
        self.shards[self.index(address)]
            .write()
            .unwrap()
            .entry(address)
            .or_insert(account);
    }

    /// Inserts the slot into its (address, key) shard unless it is already there. In the
    /// account's own shard it is skipped while the account has no entry, so a cached slot never
    /// stands in for the account.
    fn cache_slot(&self, address: Address, key: U256, slot: EvmStorageSlot) {
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

    /// Merges all shards into one `EvmState`.
    fn into_evm_state(self) -> EvmState {
        self.into_unlocked().into_inner()
    }
}

#[cfg(test)]
#[path = "tests/sharded.rs"]
mod tests;
