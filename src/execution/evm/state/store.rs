//! The storage behind a `StateOverlay`: the read and write interfaces, and their implementation for
//! a plain `EvmState`, used sequentially. `Sharded` implements them for shared use in `sharded/`.

use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, Bytecode, EvmState, EvmStorageSlot},
};
use alloy_primitives::{Address, B256, U256};

use super::StateDb;
use crate::execution::slot::CrdtSlot;
use crate::store::traits::StoreError;

/// Read side of the storage behind a `StateOverlay`; `StateDb` reads through it.
pub trait OverlayRead {
    /// Reads `address`'s account; its storage may hold only part of the account's slots.
    fn account<R>(&self, address: Address, f: impl FnOnce(Option<&Account>) -> R) -> R;

    /// Reads `address`'s slot `key`.
    fn slot<R>(
        &self,
        address: Address,
        key: U256,
        f: impl FnOnce(Option<&EvmStorageSlot>) -> R,
    ) -> R;

    /// Returns the code with `code_hash` held by any account.
    fn code(&self, code_hash: B256) -> Option<Bytecode>;

    /// Keeps `account`, loaded from the fallback, unless `address` already has an entry; only a
    /// shared store caches.
    fn cache_account(&self, address: Address, account: Account);

    /// Keeps `slot`, loaded from the fallback, unless `address`'s slot `key` already exists; only a
    /// shared store caches.
    fn cache_slot(&self, address: Address, key: U256, slot: EvmStorageSlot);

    /// Runs before reading `address`'s slot `key`: a shared store converts the Set the key belongs
    /// to into the execution format on its first read, loading missing slots with `load`; other
    /// stores do nothing.
    fn convert_on_read(&self, address: Address, key: U256, load: impl Fn(U256) -> Option<U256>);
}

/// Storage behind a `StateOverlay`: a plain `EvmState` for sequential use, or a
/// `sharded::Sharded` state used by parallel sequences.
pub trait OverlayWrite: OverlayRead + Sized {
    /// Updates `address`'s account info, inserting `init()` if absent; slots go through
    /// `slot_mut`.
    fn account_mut<R>(
        &mut self,
        address: Address,
        init: impl FnOnce() -> Account,
        f: impl FnOnce(&mut Account) -> R,
    ) -> R;

    /// Updates `address`'s slot `key`, inserting `init()` if absent.
    fn slot_mut<R>(
        &mut self,
        address: Address,
        key: U256,
        init: impl FnOnce() -> EvmStorageSlot,
        f: impl FnOnce(&mut EvmStorageSlot) -> R,
    ) -> R;

    /// Returns all accounts as one `EvmState`.
    fn into_evm_state(self) -> EvmState;

    /// Writes merged `accounts`: each account's info replaces the stored info, its slots replace
    /// or add to the stored ones; serially for a sequential store, lock-free in parallel for a
    /// shared one.
    fn write_accounts(&mut self, accounts: EvmState);

    /// Applies `crdt`'s collected deltas into `db` and clears them: owner after owner for a
    /// sequential store, all owners in parallel for a shared one.
    fn apply_crdt<DB: DatabaseRef + Sync>(
        db: &mut StateDb<DB, Self>,
        crdt: &mut CrdtSlot,
    ) -> Result<(), StoreError>;
}

impl<T: OverlayRead> OverlayRead for &T {
    /// Reads through the referenced store.
    fn account<R>(&self, address: Address, f: impl FnOnce(Option<&Account>) -> R) -> R {
        (**self).account(address, f)
    }

    /// Reads through the referenced store.
    fn slot<R>(
        &self,
        address: Address,
        key: U256,
        f: impl FnOnce(Option<&EvmStorageSlot>) -> R,
    ) -> R {
        (**self).slot(address, key, f)
    }

    /// Reads through the referenced store.
    fn code(&self, code_hash: B256) -> Option<Bytecode> {
        (**self).code(code_hash)
    }

    /// Caches through the referenced store.
    fn cache_account(&self, address: Address, account: Account) {
        (**self).cache_account(address, account)
    }

    /// Caches through the referenced store.
    fn cache_slot(&self, address: Address, key: U256, slot: EvmStorageSlot) {
        (**self).cache_slot(address, key, slot)
    }

    /// Converts through the referenced store.
    fn convert_on_read(&self, address: Address, key: U256, load: impl Fn(U256) -> Option<U256>) {
        (**self).convert_on_read(address, key, load)
    }
}

impl OverlayRead for EvmState {
    /// Reads the account entry.
    fn account<R>(&self, address: Address, f: impl FnOnce(Option<&Account>) -> R) -> R {
        f(self.get(&address))
    }

    /// Reads the slot from the account's storage.
    fn slot<R>(
        &self,
        address: Address,
        key: U256,
        f: impl FnOnce(Option<&EvmStorageSlot>) -> R,
    ) -> R {
        f(self.get(&address).and_then(|account| account.storage.get(&key)))
    }

    /// Searches every account for the code.
    fn code(&self, code_hash: B256) -> Option<Bytecode> {
        self.values()
            .filter(|account| account.info.code_hash == code_hash)
            .find_map(|account| account.info.code.clone())
    }

    /// Does nothing: a sequential state is not shared, so it does not cache reads.
    fn cache_account(&self, _address: Address, _account: Account) {}

    /// Does nothing: a sequential state is not shared, so it does not cache reads.
    fn cache_slot(&self, _address: Address, _key: U256, _slot: EvmStorageSlot) {}

    /// Does nothing: a sequential state leaves Set conversion to the Set handler.
    fn convert_on_read(&self, _address: Address, _key: U256, _load: impl Fn(U256) -> Option<U256>) {}
}

impl OverlayWrite for EvmState {
    /// Updates the account entry, inserting `init()` if absent.
    fn account_mut<R>(
        &mut self,
        address: Address,
        init: impl FnOnce() -> Account,
        f: impl FnOnce(&mut Account) -> R,
    ) -> R {
        f(self.entry(address).or_insert_with(init))
    }

    /// Updates the slot in the account's storage, inserting `init()` if absent.
    fn slot_mut<R>(
        &mut self,
        address: Address,
        key: U256,
        init: impl FnOnce() -> EvmStorageSlot,
        f: impl FnOnce(&mut EvmStorageSlot) -> R,
    ) -> R {
        f(self
            .entry(address)
            .or_default()
            .storage
            .entry(key)
            .or_insert_with(init))
    }

    /// Returns the state itself.
    fn into_evm_state(self) -> EvmState {
        self
    }

    /// Replaces each account's info, keeping its stored slots, then writes its slots.
    fn write_accounts(&mut self, accounts: EvmState) {
        for (address, mut account) in accounts {
            let storage = std::mem::take(&mut account.storage);
            let current = self.entry(address).or_default();
            account.storage = std::mem::take(&mut current.storage);
            *current = account;
            current.storage.extend(storage);
        }
    }

    /// Applies the deltas owner after owner through `db`.
    fn apply_crdt<DB: DatabaseRef + Sync>(
        db: &mut StateDb<DB, Self>,
        crdt: &mut CrdtSlot,
    ) -> Result<(), StoreError> {
        crdt.apply(db)
    }
}
