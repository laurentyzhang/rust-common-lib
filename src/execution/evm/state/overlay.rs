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

    /// Applies incoming account and storage changes to the overlay.
    pub(super) fn merge<DB: DatabaseRef>(
        &mut self,
        incoming: EvmState,
        fallback: Option<&DB>,
    ) -> Result<(), StoreError> {
        // Step 1 (serial): pair each incoming account with its committed counterpart.
        let accounts = incoming
            .into_iter()
            .map(|(address, incoming)| {
                Ok::<_, StoreError>((address, self.take_current(address, fallback)?, incoming))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Step 2 (parallel): accounts are independent, update then in parallel.
        let merged_accounts: Vec<_> = accounts
            .into_par_iter()
            .map(|(address, mut current, incoming)| {
                if current == Account::default() || incoming.is_selfdestructed_locally() {
                    return Ok((address, incoming));
                }

                Self::handle_fee_nonce(&mut current, &incoming).map_err(StoreError::State)?;
                Self::merge_status(&mut current, &incoming);

                // Both accounts exist, merge their storage.
                for (key, value) in incoming.storage {
                    if ApiAddress::is_storage_key(key) {
                        Self::merge_api_slot(&mut current.storage, key, value);
                    } else {
                        current.storage.insert(key, value);
                    }
                }
                Ok((address, current))
            })
            .collect::<Result<_, StoreError>>()?;

        // Step 3 (serial): put the merged accounts back into the overlay.
        self.state.extend(merged_accounts);
        Ok(())
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

    /// Takes the committed account out of the map so workers own it; it is re-inserted after merging.
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

    /// Handles the fee and nonce fields for an account by applying the delta between the original and incoming values.
    fn handle_fee_nonce(current: &mut Account, incoming: &Account) -> Result<(), CrdtError> {
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
}
