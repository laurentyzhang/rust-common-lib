//! Routes the CRDT slots of EVM states to their namespaces' appliers.

use std::fmt::Debug;

use alloy_evm::revm::state::{EvmState, EvmStorage};
use alloy_primitives::map::HashMap;
use alloy_primitives::{Address, U256};
use rayon::prelude::*;

use super::{ApplyDelta, scalar, set};
use crate::common::codec::CodecError;
use crate::execution::evm::api_store::ApiStore;
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
    pub(crate) fn merge_delta(&mut self, id: ID, mut incoming: EvmState) -> Result<EvmState, CodecError> {
        for (owner, slots) in Self::take_if(&mut incoming, |key| set::KeyCodec::decode(key).is_some()) {
            set::Applier::merge_delta(self.sets.entry(owner).or_default(), id, &slots)?;
        }
        for (owner, slots) in Self::take_if(&mut incoming, |key| scalar::KeyCodec::decode(key).is_some()) {
            scalar::Applier::merge_delta(self.scalars.entry(owner).or_default(), id, &slots)?;
        }
        Ok(incoming)
    }

    /// Applies the collected deltas onto `store` and clears them.
    pub(crate) fn apply<S: ApiStore>(&mut self, store: &mut S) -> Result<(), StoreError>
    where
        S::Error: Debug,
    {
        for (owner, delta) in self.sets.drain() {
            set::Applier::apply(store, owner, delta)?;
        }
        for (owner, delta) in self.scalars.drain() {
            scalar::Applier::apply(store, owner, delta)?;
        }
        Ok(())
    }

    /// Applies each owner's deltas on its own thread, into the store `store_for` makes for that
    /// owner, and clears them; returns the stores. Owners are independent, so none reads another's
    /// writes.
    pub(crate) fn apply_by_owner<S>(
        &mut self,
        store_for: impl Fn(Address) -> S + Sync,
    ) -> Result<Vec<S>, StoreError>
    where
        S: ApiStore + Send,
        S::Error: Debug,
    {
        let mut owners = HashMap::<Address, (Option<set::Delta>, Option<scalar::Delta>)>::default();
        for (owner, delta) in self.sets.drain() {
            owners.entry(owner).or_default().0 = Some(delta);
        }
        for (owner, delta) in self.scalars.drain() {
            owners.entry(owner).or_default().1 = Some(delta);
        }
        owners
            .into_iter()
            .collect::<Vec<_>>()
            .into_par_iter()
            .map(|(owner, (sets, scalars))| {
                let mut store = store_for(owner);
                if let Some(delta) = sets {
                    set::Applier::apply(&mut store, owner, delta)?;
                }
                if let Some(delta) = scalars {
                    scalar::Applier::apply(&mut store, owner, delta)?;
                }
                Ok(store)
            })
            .collect()
    }

    /// Applies the CRDT slots of `incoming` alone onto `store`; returns the plain slots.
    pub(crate) fn apply_sequential<S: ApiStore>(store: &mut S, incoming: EvmState) -> Result<EvmState, StoreError>
    where
        S::Error: Debug,
    {
        let mut crdt = Self::default();
        let native = crdt.merge_delta(ID::default(), incoming)?;
        crdt.apply(store)?;
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
