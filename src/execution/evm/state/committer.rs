use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, EvmState, EvmStorage},
};
use alloy_primitives::{Address, U256};
use std::io::Error;

use super::{StateDb, overlay::StateOverlay};
use crate::crdt::state::CrdtError;
use crate::execution::workload::ID;
use crate::execution::{DeltaMerger, scalar, set};
use crate::store::traits::{Committer, StoreError};

/// Buffers transaction states and commits accepted changes.
#[derive(Debug)]
pub struct EvmStateCommitter {
    state: StateDb,
    buffered: BTreeMap<(u64, u64, u64, u64, u64), EvmState>,
    nonce_fee_handler: fn(&mut Account, &Account) -> Result<(), CrdtError>,
}

impl EvmStateCommitter {
    pub fn new_sequential() -> Self {
        Self {
            state: StateDb::default(),
            buffered: BTreeMap::new(),
            nonce_fee_handler: StateOverlay::copy_balance_nonce,
        }
    }

    pub fn new_parallel() -> Self {
        Self {
            state: StateDb::default(),
            buffered: BTreeMap::new(),
            nonce_fee_handler: StateOverlay::add_balance_nonce_deltas,
        }
    }

    pub fn with_target(target: EvmState) -> Self {
        Self {
            state: StateDb::new(target),
            ..Self::new_parallel()
        }
    }

    fn key(id: ID) -> (u64, u64, u64, u64, u64) {
        (id.block, id.generation, id.sequence, id.job, id.global)
    }

    pub fn inner(&self) -> &EvmState {
        self.state.inner()
    }

    pub fn into_inner(self) -> EvmState {
        self.state.into_inner()
    }

    /// Merges the buffered states against the shared base; CRDT slots go through their mergers.
    pub(crate) fn commit<DB: DatabaseRef>(self, fallback: &DB) -> Result<EvmState, Error> {
        let overlay = StateOverlay::new(self.state.into_inner(), self.nonce_fee_handler);
        let mut db = StateDb::with_fallback(overlay, Some(fallback));
        for incoming in self.buffered.into_values() {
            Self::merge(&mut db, incoming).map_err(Error::other)?;
        }
        Ok(db.into_inner())
    }

    /// Applies one state to `db`: plain slots through the overlay, Set and scalar slots through
    /// their mergers.
    fn merge<DB: DatabaseRef>(db: &mut StateDb<DB>, mut incoming: EvmState) -> Result<(), StoreError> {
        let sets = Self::split(&mut incoming, |key| set::KeyCodec::decode(key).is_some());
        let scalars = Self::split(&mut incoming, |key| scalar::KeyCodec::decode(key).is_some());
        db.overlay.apply(incoming, db.fallback.as_ref())?;
        for (owner, slots) in sets {
            set::Merger::merge(db, owner, &slots)?;
        }
        for (owner, slots) in scalars {
            scalar::Merger::merge(db, owner, &slots)?;
        }
        Ok(())
    }

    /// Moves each account's slots selected by `belongs` out of `incoming`, for a CRDT merger.
    pub(crate) fn split(incoming: &mut EvmState, belongs: impl Fn(U256) -> bool) -> Vec<(Address, EvmStorage)> {
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

impl Committer<ID, EvmState> for EvmStateCommitter {
    fn buffer(&mut self, id: ID, state: EvmState) {
        self.buffered.insert(Self::key(id), state);
    }

    fn stage(&mut self, accepted: Vec<ID>) {
        let accepted = accepted.into_iter().map(Self::key).collect::<HashSet<_>>();
        self.buffered.retain(|id, _| accepted.contains(id));
    }

    /// Merges the buffered states into the target; CRDT slots go through their mergers.
    fn flush(&mut self) -> Result<(), StoreError> {
        let overlay = StateOverlay::new(self.state.inner().clone(), self.nonce_fee_handler);
        let mut db = StateDb::with_fallback(overlay, self.state.fallback);
        for incoming in self.buffered.values() {
            Self::merge(&mut db, incoming.clone())?;
        }
        self.state.overlay = db.overlay;
        self.buffered.clear();
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/committer.rs"]
mod tests;
