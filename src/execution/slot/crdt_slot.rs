//! Routes the CRDT slots of EVM states to their namespaces' appliers.

use std::fmt::Debug;

use alloy_evm::revm::DatabaseRef;
use alloy_evm::revm::state::{EvmState, EvmStorage};
use alloy_primitives::map::HashMap;
use alloy_primitives::{Address, U256};

use super::{ApplyDelta, scalar, set};
use crate::common::codec::CodecError;
use crate::execution::StateDb;
use crate::execution::evm::api_store::ApiStore;
use crate::execution::evm::state::store::OverlayWrite;
use crate::execution::workload::ID;
use crate::store::StoreError;

/// CRDT changes of accepted states, per owner and namespace, waiting to be applied once.
#[derive(Debug, Default)]
pub struct CrdtSlot {
    sets: HashMap<Address, set::Delta>,
    scalars: HashMap<Address, scalar::Delta>,
}

impl CrdtSlot {
    /// Moves the Set and scalar slots out of `incoming` into the collected deltas, tagged with
    /// `id`; returns `incoming` with only the plain slots left.
    pub(crate) fn merge_delta_from(
        &mut self,
        id: ID,
        mut incoming: EvmState,
    ) -> Result<EvmState, CodecError> {
        for (owner, slots) in
            Self::take_if(&mut incoming, |key| set::KeyCodec::decode(key).is_some())
        {
            set::Applier::merge_delta_from(self.sets.entry(owner).or_default(), id, &slots)?;
        }
        for (owner, slots) in
            Self::take_if(&mut incoming, |key| scalar::KeyCodec::decode(key).is_some())
        {
            scalar::Applier::merge_delta_from(self.scalars.entry(owner).or_default(), id, &slots)?;
        }
        Ok(incoming)
    }

    /// Applies the collected deltas into `db` and clears them: owners in parallel against a view
    /// of the store, then one batched write.
    pub(crate) fn apply<DB: DatabaseRef + Sync, Store: OverlayWrite>(
        &mut self,
        db: &mut StateDb<DB, Store>,
    ) -> Result<(), StoreError> {
        crate::execution::evm::state::apply_crdt(db, self)
    }

    /// Takes the collected deltas, grouped by owner, and clears them.
    pub(crate) fn drain(&mut self) -> Vec<(Address, Option<set::Delta>, Option<scalar::Delta>)> {
        let mut owners = HashMap::<Address, (Option<set::Delta>, Option<scalar::Delta>)>::default();
        for (owner, delta) in self.sets.drain() {
            owners.entry(owner).or_default().0 = Some(delta);
        }
        for (owner, delta) in self.scalars.drain() {
            owners.entry(owner).or_default().1 = Some(delta);
        }
        owners
            .into_iter()
            .map(|(owner, (sets, scalars))| (owner, sets, scalars))
            .collect()
    }

    /// Applies one owner's Set and scalar deltas into `store`.
    pub(crate) fn apply_owner<S: ApiStore>(
        store: &mut S,
        owner: Address,
        sets: Option<set::Delta>,
        scalars: Option<scalar::Delta>,
    ) -> Result<(), StoreError>
    where
        S::Error: Debug,
    {
        if let Some(delta) = sets {
            set::Applier::apply(store, owner, delta)?;
        }
        if let Some(delta) = scalars {
            scalar::Applier::apply(store, owner, delta)?;
        }
        Ok(())
    }

    /// Applies the CRDT slots of `incoming` alone into `db`; returns the plain slots.
    pub(crate) fn merge_and_apply<DB: DatabaseRef + Sync, Store: OverlayWrite>(
        db: &mut StateDb<DB, Store>,
        incoming: EvmState,
    ) -> Result<EvmState, StoreError> {
        let mut crdt = Self::default();
        let native = crdt.merge_delta_from(ID::default(), incoming)?;
        crdt.apply(db)?;
        Ok(native)
    }

    /// Moves each account's slots selected by `belongs` out of `incoming`, for a CRDT applier.
    pub(crate) fn take_if(
        incoming: &mut EvmState,
        belongs: impl Fn(U256) -> bool,
    ) -> Vec<(Address, EvmStorage)> {
        incoming
            .iter_mut()
            .filter_map(|(&owner, account)| {
                let mut slots = EvmStorage::default();
                account.storage.retain(|&key, slot| {
                    if !belongs(key) {
                        return true;
                    }
                    slots.insert(key, slot.clone());
                    false
                });
                (!slots.is_empty()).then_some((owner, slots))
            })
            .collect()
    }
}
