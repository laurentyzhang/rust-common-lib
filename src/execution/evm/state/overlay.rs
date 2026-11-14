use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, EvmState, EvmStorageSlot},
};
use alloy_primitives::{Address, U256};
use rayon::prelude::*;

use crate::crdt::state::CrdtError;
use crate::crdt::{Crdt, Scalar, Type};
use crate::execution::{ApiAddress, Property, ScalarKeyCodec};
use crate::store::traits::StoreError;

#[derive(Debug)]
pub struct StateOverlay {
    state: EvmState,
    balance_nonce_handler: fn(&mut Account, &Account) -> Result<(), CrdtError>,
}

impl StateOverlay {
    pub fn new(
        state: EvmState,
        balance_nonce_handler: fn(&mut Account, &Account) -> Result<(), CrdtError>,
    ) -> Self {
        Self {
            state,
            balance_nonce_handler,
        }
    }

    pub(super) fn inner(&self) -> &EvmState {
        &self.state
    }

    pub(super) fn inner_mut(&mut self) -> &mut EvmState {
        &mut self.state
    }

    pub(in crate::execution) fn into_inner(self) -> EvmState {
        self.state
    }

    /// Applies account changes using the selected balance and nonce strategy.
    pub(in crate::execution) fn apply<DB: DatabaseRef>(
        &mut self,
        incoming: EvmState,
        fallback: Option<&DB>,
    ) -> Result<(), StoreError> {
        // Step 1 (serial): pair each incoming account with its committed counterpart.
        let accounts = incoming
            .into_iter()
            .map(|(address, incoming)| {
                let mut current = self.take_current(address, fallback, &incoming)?;
                // Load missing slots before applying deltas, retaining their base values.
                for (&key, slot) in &incoming.storage {
                    if !current.storage.contains_key(&key) {
                        let original = match fallback {
                            Some(db) => db
                                .storage_ref(address, key)
                                .map_err(|error| StoreError::Database(format!("{error:?}")))?,
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
        balance_nonce_handler(&mut current, &incoming).map_err(StoreError::State)?;
        // Use the incoming account's lifecycle flags.
        Self::merge_status(&mut current, &incoming);
        if incoming.info.code_hash != incoming.original_info().code_hash {
            current.info.code_hash = incoming.info.code_hash;
            current.info.code = incoming.info.code;
        } else if incoming.info.code_hash == current.info.code_hash && incoming.info.code.is_some()
        {
            current.info.code = incoming.info.code;
        }
        current.transaction_id = incoming.transaction_id;

        // Combine API deltas; ordinary storage slots take the incoming values.
        for (key, mut value) in incoming.storage {
            if let Some(slot) = current.storage.get(&key) {
                if ApiAddress::is_storage_key(key)
                    && let Some(change) = Self::api_slot_delta(key, &value)
                {
                    value.present_value = slot.present_value().wrapping_add(change);
                }
                value.original_value = slot.original_value();
            }
            current.storage.insert(key, value);
        }
        Ok((address, current))
    }

    /// Replaces the current status with the incoming account's status.
    fn merge_status(current: &mut Account, incoming: &Account) {
        current.status = incoming.status;
    }

    pub(in crate::execution) fn api_slot_delta(key: U256, slot: &EvmStorageSlot) -> Option<U256> {
        match ScalarKeyCodec::decode(key) {
            Some((_, field, _))
                if field == Property::PosDelta as u8 || field == Property::NegDelta as u8 =>
            {
                Some(slot.present_value().wrapping_sub(slot.original_value()))
            }
            _ => None,
        }
    }

    /// Clones the current account from the overlay or loads it from the fallback.
    fn take_current<DB: DatabaseRef>(
        &self,
        address: Address,
        fallback: Option<&DB>,
        incoming: &Account,
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
            None => Ok(Account::from(incoming.original_info())),
        }
    }

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
