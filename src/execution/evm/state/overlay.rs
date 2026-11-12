use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, EvmState, EvmStorage, EvmStorageSlot},
};
use alloy_primitives::{Address, U256};
use rayon::prelude::*;

use crate::crdt::state::{CrdtError, NumericError};
use crate::crdt::{Crdt, Scalar, Type};
use crate::execution::{ApiAddress, Property, ScalarKeyCodec};
use crate::store::traits::StoreError;

#[derive(Debug, Default)]
pub(super) struct StateOverlay {
    state: EvmState,
}

impl StateOverlay {
    pub(super) fn new(state: EvmState) -> Self {
        Self { state }
    }

    pub(super) fn inner(&self) -> &EvmState {
        &self.state
    }

    pub(super) fn inner_mut(&mut self) -> &mut EvmState {
        &mut self.state
    }

    pub(super) fn into_inner(self) -> EvmState {
        self.state
    }

    /// Applies account changes using the selected balance and nonce strategy.
    pub(super) fn apply<DB: DatabaseRef>(
        &mut self,
        incoming: EvmState,
        fallback: Option<&DB>,
        balance_nonce_handler: fn(&mut Account, &Account) -> Result<(), CrdtError>,
    ) -> Result<(), StoreError> {
        // Step 1 (serial): pair each incoming account with its committed counterpart.
        let accounts = incoming
            .into_iter()
            .map(|(address, incoming)| {
                Ok::<_, StoreError>((address, self.take_current(address, fallback)?, incoming))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Step 2 (parallel): merge independent accounts without changing the overlay.
        let merged_accounts: Vec<_> = accounts
            .into_par_iter()
            .map(|(address, current, incoming)| {
                Self::merge_account(address, current, incoming, balance_nonce_handler)
            })
            .collect::<Result<_, StoreError>>()?;

        // Step 3 (serial): put the merged accounts back into the overlay.
        self.state.extend(merged_accounts);
        Ok(())
    }

    /// Merges one account's balance, nonce, status, and storage.
    fn merge_account(
        address: Address,
        mut current: Account,
        incoming: Account,
        balance_nonce_handler: fn(&mut Account, &Account) -> Result<(), CrdtError>,
    ) -> Result<(Address, Account), StoreError> {
        // An empty current account takes the incoming account without invoking the strategy.
        if current == Account::default() {
            return Ok((address, incoming));
        }

        balance_nonce_handler(&mut current, &incoming).map_err(StoreError::State)?;
        // Use the incoming account's lifecycle flags.
        Self::merge_status(&mut current, &incoming);

        // Combine API deltas; ordinary storage slots take the incoming values.
        for (key, value) in incoming.storage {
            if ApiAddress::is_storage_key(key) {
                Self::merge_api_slot(&mut current.storage, key, value);
            } else {
                current.storage.insert(key, value);
            }
        }
        Ok((address, current))
    }

    /// Replaces the current status with the incoming account's status.
    fn merge_status(current: &mut Account, incoming: &Account) {
        current.status = incoming.status;
    }

    /// Dispatches CRDT delta merging by API type.
    fn merge_api_slot(storage: &mut EvmStorage, key: U256, new_value: EvmStorageSlot) {
        match ScalarKeyCodec::decode(key) {
            Some((_, field, _))
                if field == Property::PosDelta as u8 || field == Property::NegDelta as u8 =>
            {
                // Apply only this state's change to the delta magnitude.
                let change = new_value
                    .present_value()
                    .wrapping_sub(new_value.original_value());
                if let Some(slot) = storage.get_mut(&key) {
                    slot.present_value = slot.present_value().wrapping_add(change);
                } else {
                    storage.insert(key, new_value);
                }
            }
            _ => {}
        }
    }

    /// Clones the current account from the overlay or loads it from the fallback.
    fn take_current<DB: DatabaseRef>(
        &self,
        address: Address,
        fallback: Option<&DB>,
    ) -> Result<Account, StoreError> {
        if let Some(current) = self.state.get(&address) {
            return Ok(current.clone());
        }

        match fallback {
            Some(db) => Ok(
                match db
                    .basic_ref(address)
                    .map_err(|error| StoreError::Database(format!("{error:?}")))?
                {
                    Some(info) => Account::from(info),
                    None => Account::default(),
                },
            ),
            None => Ok(Account::default()),
        }
    }

    /// Adds the incoming balance and nonce changes to the current account.
    pub(super) fn add_balance_nonce_deltas(
        current: &mut Account,
        incoming: &Account,
    ) -> Result<(), CrdtError> {
        let original = incoming.original_info();

        let balance = Scalar::with_delta(
            current.info.balance,
            U256::delta_between(original.balance, incoming.info.balance),
        )?
        .apply_delta()
        .value;

        let merged_nonce = Scalar::with_delta(
            U256::from(current.info.nonce),
            U256::delta_between(U256::from(original.nonce), U256::from(incoming.info.nonce)),
        )?
        .apply_delta()
        .value;

        let nonce = u64::try_from(merged_nonce).map_err(|_| {
            CrdtError::U256(NumericError::Overflow("merged nonce exceeds u64".into()))
        })?;

        current.info.balance = balance;
        current.info.nonce = nonce;
        Ok(())
    }

    /// Copies the incoming balance and nonce from a subsequent state.
    pub(super) fn copy_balance_nonce(
        current: &mut Account,
        incoming: &Account,
    ) -> Result<(), CrdtError> {
        current.info.balance = incoming.info.balance;
        current.info.nonce = incoming.info.nonce;
        Ok(())
    }
}
