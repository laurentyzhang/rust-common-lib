use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, AccountStatus, EvmState, EvmStorage, EvmStorageSlot},
};
use alloy_primitives::{Address, U256};
use rayon::prelude::*;

use crate::crdt::{Crdt, Scalar, Type};
use crate::execution::{ApiAddress, Property, ScalarKeyCodec};

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
    ) -> Result<(), DB::Error> {
        // Step 1 (serial): pair each incoming account with its committed counterpart.
        let accounts = incoming
            .into_iter()
            .map(|(address, incoming)| {
                Ok((
                    address,
                    self.take_current(address, &incoming, fallback)?,
                    incoming,
                ))
            })
            .collect::<Result<Vec<_>, _>>()?;

        // Step 2 (parallel): accounts are independent; only successive incoming states require ordering.
        let merged: Vec<_> = accounts
            .into_par_iter()
            .map(|(address, current, incoming)| (address, Self::merge_account(current, incoming)))
            .collect();
        // Step 3 (serial): put the merged accounts back into the overlay.
        self.state.extend(merged);
        Ok(())
    }

    /// Takes the committed account out of the map so workers own it; it is re-inserted after merging.
    fn take_current<DB: DatabaseRef>(
        &mut self,
        address: Address,
        incoming: &Account,
        fallback: Option<&DB>,
    ) -> Result<Account, DB::Error> {
        if let Some(current) = self.state.remove(&address) {
            return Ok(current);
        }
        // Not committed yet: build a baseline account from the fallback DB (info only, no code/storage).
        let info = match fallback {
            Some(db) => db.basic_ref(address)?.unwrap_or_default(),
            // The incoming snapshot supplies the base when no fallback is attached.
            None => incoming.original_info(),
        };
        let mut current = Account::from(info);
        current.info.code = None;
        Ok(current)
    }

    /// Merges one incoming account into its committed counterpart.
    fn merge_account(mut current: Account, mut incoming: Account) -> Account {
        // `base` is the committed state's original info, kept as the merged account's original.
        // `original` is what the incoming state started from, used to compute its deltas.
        let base = current.original_info();
        let original = incoming.original_info();
        // A reset discards committed storage: the account was destroyed or (re)created.
        let reset =
            current.is_selfdestructed() || incoming.is_created() || incoming.is_selfdestructed();

        if current.is_selfdestructed() {
            current.info = Default::default();
        }

        // Balance and nonce are commutative: apply (incoming - original) onto the committed value
        // instead of overwriting it, so concurrent changes from other states are preserved.
        let mut nonce = U256::from(current.info.nonce);
        for (value, old, new) in [
            (
                &mut current.info.balance,
                original.balance,
                incoming.info.balance,
            ),
            (
                &mut nonce,
                U256::from(original.nonce),
                U256::from(incoming.info.nonce),
            ),
        ] {
            *value = Scalar::with_delta(*value, U256::delta_between(old, new))
                .unwrap()
                .apply_delta()
                .value;
        }

        current.info.nonce = nonce.to::<u64>();
        // Code is only replaced when the incoming state deploys some.
        if let Some(code) = incoming.info.code.take() {
            current.info.code_hash = incoming.info.code_hash;
            current.info.code = Some(code);
        }
        if !reset {
            // Keep the creation flag from the committed state.
            if current.is_created() {
                incoming.mark_created();
            }
            Self::merge_storage(&mut current.storage, &mut incoming);
        }
        // The merged account keeps incoming metadata (status, etc.) with the combined info/storage.
        incoming.info = current.info;
        *incoming.original_info_mut() = base;
        // A touched, surviving account exists now, so it is no longer "loaded as not existing".
        if incoming.is_touched() && !incoming.is_selfdestructed() {
            incoming.status.remove(AccountStatus::LoadedAsNotExisting);
        }
        incoming
    }

    /// Overlays incoming slots on committed ones and stores the result in `incoming.storage`.
    /// CRDT API slots that already exist are merged as deltas instead of being overwritten.
    fn merge_storage(committed: &mut EvmStorage, incoming: &mut Account) {
        for (key, value) in std::mem::take(&mut incoming.storage) {
            if !ApiAddress::is_storage_key(key) || !committed.contains_key(&key) {
                committed.insert(key, value);
            } else {
                Self::handle_api_slot(committed, key, value);
            }
        }
        incoming.storage = std::mem::take(committed);
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
