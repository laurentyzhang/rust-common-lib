use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, AccountStatus, Bytecode, EvmState, EvmStorageSlot},
};
use alloy_primitives::{Address, B256, U256};
use rayon::prelude::*;

use crate::crdt::state::CrdtError;
use crate::crdt::{Crdt, Scalar, Type};
use crate::store::traits::StoreError;

/// Storage behind a `StateOverlay`: a plain `EvmState` for sequential use, or a
/// `sharded::Shared` state used by parallel sequences.
pub trait OverlayState {
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

    /// Keeps `account`, loaded from the fallback, unless `address` already has an entry; only a
    /// shared store caches.
    fn cache_account(&self, address: Address, account: Account);

    /// Keeps `slot`, loaded from the fallback, unless `address`'s slot `key` already exists; only a
    /// shared store caches.
    fn cache_slot(&self, address: Address, key: U256, slot: EvmStorageSlot);

    /// Returns all accounts as one `EvmState`.
    fn into_evm_state(self) -> EvmState;
}

impl OverlayState for EvmState {
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

    /// Does nothing: a sequential state is not shared, so it does not cache reads.
    fn cache_account(&self, _address: Address, _account: Account) {}

    /// Does nothing: a sequential state is not shared, so it does not cache reads.
    fn cache_slot(&self, _address: Address, _key: U256, _slot: EvmStorageSlot) {}

    /// Returns the state itself.
    fn into_evm_state(self) -> EvmState {
        self
    }
}

#[derive(Debug)]
pub struct StateOverlay<Store = EvmState> {
    state: Store,
    balance_nonce_handler: fn(&mut Account, &Account) -> Result<(), CrdtError>,
}

impl<Store> StateOverlay<Store> {
    /// Wraps `state` with the balance and nonce strategy used when applying changes.
    pub fn new(
        state: Store,
        balance_nonce_handler: fn(&mut Account, &Account) -> Result<(), CrdtError>,
    ) -> Self {
        Self {
            state,
            balance_nonce_handler,
        }
    }

    pub(super) fn inner(&self) -> &Store {
        &self.state
    }

    pub(super) fn inner_mut(&mut self) -> &mut Store {
        &mut self.state
    }
}

impl<Store: OverlayState> StateOverlay<Store> {
    /// Returns all accounts as one `EvmState`.
    pub(in crate::execution) fn into_inner(self) -> EvmState {
        self.state.into_evm_state()
    }

    /// Applies account changes using the selected balance and nonce strategy.
    pub(in crate::execution) fn apply<DB: DatabaseRef>(
        &mut self,
        incoming: EvmState,
        fallback: Option<&DB>,
    ) -> Result<(), StoreError> {
        self.apply_with_fallbacks(incoming, fallback, fallback)
    }

    /// Applies changes using the supplied fallbacks for accounts and storage slots.
    pub(super) fn apply_with_fallbacks<DB: DatabaseRef>(
        &mut self,
        incoming: EvmState,
        fallback: Option<&DB>,
        slot_fallback: Option<&DB>,
    ) -> Result<(), StoreError> {
        // Step 1 (serial): pair each incoming account with its committed counterpart.
        let accounts = incoming
            .into_iter()
            .map(|(address, incoming)| {
                let mut current = self.take_current(address, fallback, &incoming)?;
                // Take the incoming slots from the overlay, or load them keeping their base values.
                for (&key, slot) in &incoming.storage {
                    if let Some(stored) = self.state.slot(address, key, |stored| stored.cloned()) {
                        current.storage.insert(key, stored);
                    } else {
                        let original = match slot_fallback {
                            Some(db) => db
                                .storage_ref(address, key)
                                .map_err(|error| {
                                    StoreError::Database(format!(
                                        "failed to read account {address} storage slot {key:#x} while applying EVM state. Cause {error:?}"
                                    ))
                                })?,
                            None => slot.original_value(),
                        };
                        current
                            .storage
                            .insert(key, EvmStorageSlot::new(original, slot.transaction_id));
                    }
                }
                Ok::<_, StoreError>((address, current, incoming))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Step 2 (parallel): merge independent accounts without changing the overlay.
        let merged_accounts: Vec<_> = accounts
            .into_par_iter()
            .map(|(address, current, incoming)| {
                Self::merge_account(address, current, incoming, self.balance_nonce_handler)
            })
            .collect::<Result<_, StoreError>>()?;

        // Step 3 (serial): put the merged accounts and their slots back into the overlay.
        for (address, mut account) in merged_accounts {
            let storage = std::mem::take(&mut account.storage);
            // Replace the account but keep the slots already stored with it.
            self.state.account_mut(address, Account::default, |current| {
                account.storage = std::mem::take(&mut current.storage);
                *current = account;
            });
            for (key, slot) in storage {
                self.state
                    .slot_mut(address, key, Default::default, |current| *current = slot);
            }
        }
        Ok(())
    }

    /// Merges one account's balance, nonce, status, and storage.
    fn merge_account(
        address: Address,
        mut current: Account,
        incoming: Account,
        balance_nonce_handler: fn(&mut Account, &Account) -> Result<(), CrdtError>,
    ) -> Result<(Address, Account), StoreError> {
        let current_balance = current.info.balance; // For error msgs only
        let current_nonce = current.info.nonce;
        balance_nonce_handler(&mut current, &incoming).map_err(|error| {
            let original = incoming.original_info();
            StoreError::Transition(format!(
                "failed to apply balance/nonce transition for account {address}. \
                 Baseline balance {current_balance}, nonce {current_nonce}. \
                 Incoming balance {} -> {}, nonce {} -> {}. Cause {error:?}",
                original.balance, incoming.info.balance, original.nonce, incoming.info.nonce,
            ))
        })?;
        // Merge account status across jobs.
        Self::merge_status(&mut current, &incoming);
        if incoming.info.code_hash != incoming.original_info().code_hash {
            current.info.code_hash = incoming.info.code_hash;
            current.info.code = incoming.info.code;
        } else if incoming.info.code_hash == current.info.code_hash && incoming.info.code.is_some()
        {
            current.info.code = incoming.info.code;
        }
        current.transaction_id = incoming.transaction_id;

        // Storage slots take the incoming values; CRDT slots are merged by their mergers.
        for (key, mut value) in incoming.storage {
            if let Some(slot) = current.storage.get(&key) {
                value.original_value = slot.original_value();
            }
            current.storage.insert(key, value);
        }
        Ok((address, current))
    }

    /// Preserves accumulated account status and updates whether the account exists.
    fn merge_status(current: &mut Account, incoming: &Account) {
        // Later reads must retain earlier writes, creation, and deletion markers.
        let persistent_flags =
            AccountStatus::Touched | AccountStatus::Created | AccountStatus::SelfDestructed;
        current.status = (current.status & persistent_flags) | incoming.status;

        // Discard bookkeeping that only applies within one transaction.
        current.status.remove(
            AccountStatus::Cold | AccountStatus::CreatedLocal | AccountStatus::SelfDestructedLocal,
        );

        // A nonempty merged account now exists, even if it was missing when loaded.
        if !current.is_empty() {
            current.status.remove(AccountStatus::LoadedAsNotExisting);
        }
    }

    /// Clones the current account without its storage from the overlay, or loads it from the
    /// fallback.
    fn take_current<DB: DatabaseRef>(
        &self,
        address: Address,
        fallback: Option<&DB>,
        incoming: &Account,
    ) -> Result<Account, StoreError> {
        if let Some(current) = self.state.account(address, |current| {
            current.map(|current| {
                let mut account = Account::from(current.info.clone());
                *account.original_info_mut() = current.original_info();
                account.transaction_id = current.transaction_id;
                account.status = current.status;
                account
            })
        }) {
            return Ok(current);
        }

        match fallback {
            Some(db) => Ok(
                match db.basic_ref(address).map_err(|error| {
                    StoreError::Database(format!(
                        "failed to read account {address} while applying EVM state. Cause {error:?}"
                    ))
                })? {
                    Some(info) => Account::from(info),
                    None => Account::default(),
                },
            ),
            None => Ok(Account::from(incoming.original_info())),
        }
    }

}

impl StateOverlay {
    /// Adds the incoming balance and nonce changes to the current account.
    pub fn add_balance_nonce_deltas(
        current: &mut Account,
        incoming: &Account,
    ) -> Result<(), CrdtError> {
        // Add the balance delta to the initial account's balance.
        current.info.balance = Scalar::with_delta(
            current.info.balance,
            U256::delta_between(incoming.original_info().balance, incoming.info.balance),
        )?
        .apply_delta()
        .value;

        // Add the nonce delta to the initial account's nonce.
        current.info.nonce = Scalar::with_delta(
            current.info.nonce,
            u64::delta_between(incoming.original_info().nonce, incoming.info.nonce),
        )?
        .apply_delta()
        .value;
        Ok(())
    }

    /// Copies the incoming balance and nonce from a subsequent state.
    pub fn copy_balance_nonce(current: &mut Account, incoming: &Account) -> Result<(), CrdtError> {
        current.info.balance = incoming.info.balance;
        current.info.nonce = incoming.info.nonce;
        Ok(())
    }
}
