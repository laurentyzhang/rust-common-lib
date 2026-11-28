//! The storage behind a `StateOverlay`: the read and write interfaces, and their implementation for
//! a plain `EvmState`, used sequentially. `Sharded` implements them for shared use in `sharded/`.

use alloy_evm::revm::state::{Account, Bytecode, EvmState, EvmStorageSlot};
use alloy_primitives::{Address, B256, U256};

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
}

/// Storage behind a `StateOverlay`: a plain `EvmState` for sequential use, or a
/// `sharded::Sharded` state used by parallel sequences.
pub trait OverlayWrite: OverlayRead + Sized + Sync {
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
        f(self
            .get(&address)
            .and_then(|account| account.storage.get(&key)))
    }

    /// Searches every account for the code.
    fn code(&self, code_hash: B256) -> Option<Bytecode> {
        self.values()
            .filter(|account| account.info.code_hash == code_hash)
            .find_map(|account| account.info.code.clone())
    }
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
}

/// Merges a later batch's changes into `merged`: a created or removed account replaces the earlier
/// one, any other takes the later info and slots.
#[cfg_attr(not(feature = "mdbx"), allow(dead_code))]
pub(crate) fn merge(merged: &mut EvmState, changes: EvmState) {
    for (address, account) in changes {
        match merged.get_mut(&address) {
            Some(current) if !(account.is_created() || account.is_selfdestructed()) => {
                current.info = account.info;
                current.storage.extend(account.storage);
            }
            _ => {
                merged.insert(address, account);
            }
        }
    }
}
