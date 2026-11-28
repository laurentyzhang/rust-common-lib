use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::{DatabaseRef, database_interface::EmptyDB, state::EvmState};
use std::io::Error;

use rayon::prelude::*;

use super::{
    StateDb,
    overlay::{StateOverlay, clone_without_storage},
    store::{OverlayRead, OverlayWrite},
};
use crate::execution::slot::CrdtSlot;
use crate::execution::workload::ID;
use crate::store::traits::{Committer, StoreError};

/// Buffers transaction states and commits accepted changes.
#[derive(Debug)]
pub struct EvmStateCommitter<DB = EmptyDB, Store = EvmState> {
    /// Merged target over the readonly base; the overlay carries the balance and nonce rule.
    state: StateDb<DB, Store>,
    buffered: BTreeMap<(u64, u64, u64, u64, u64), (ID, EvmState)>,
    /// CRDT changes of the imported states, applied once by `flush` or `commit`.
    crdt: CrdtSlot,
}

impl<DB: DatabaseRef> EvmStateCommitter<DB> {
    /// Creates a committer over `readonly` whose merged balances and nonces take the incoming values.
    pub fn new_sequential(readonly: DB) -> Self {
        Self {
            state: StateDb::with_fallback(
                StateOverlay::new(EvmState::default(), StateOverlay::copy_balance_nonce),
                Some(readonly),
            ),
            buffered: BTreeMap::new(),
            crdt: CrdtSlot::default(),
        }
    }

    /// Creates a committer over `readonly` whose merged balances and nonces add up the incoming deltas.
    pub fn new_parallel(readonly: DB) -> Self {
        Self::with_target(EvmState::default(), readonly)
    }

    /// Creates a parallel committer over `readonly` that merges into `target`.
    pub fn with_target(target: EvmState, readonly: DB) -> Self {
        Self {
            state: StateDb::with_fallback(
                StateOverlay::new(target, StateOverlay::add_balance_nonce_deltas),
                Some(readonly),
            ),
            buffered: BTreeMap::new(),
            crdt: CrdtSlot::default(),
        }
    }
}

impl<DB: DatabaseRef + Sync, Store: OverlayWrite> EvmStateCommitter<DB, Store> {
    /// Creates a committer that merges into `state`, using its overlay's balance and nonce rule.
    pub fn with_state(state: StateDb<DB, Store>) -> Self {
        Self {
            state,
            buffered: BTreeMap::new(),
            crdt: CrdtSlot::default(),
        }
    }

    /// Orders buffered states by block, generation, sequence, job and global index.
    fn key(id: ID) -> (u64, u64, u64, u64, u64) {
        (id.block, id.generation, id.sequence, id.job, id.global)
    }

    /// Returns the merged target state.
    pub fn inner(&self) -> &Store {
        self.state.inner()
    }

    /// Consumes the committer and returns the merged target state.
    pub fn into_inner(self) -> EvmState {
        self.state.into_inner()
    }

    /// Merges the buffered states against the shared base and returns the merged state; nothing
    /// is written back to `readonly`.
    pub(crate) fn commit(mut self) -> Result<StateDb<DB, Store>, Error> {
        for (id, incoming) in std::mem::take(&mut self.buffered).into_values() {
            self.import(id, incoming).map_err(Error::other)?;
        }
        self.crdt.apply(&mut self.state).map_err(Error::other)?;
        Ok(self.state)
    }

    /// Imports one accepted state: its CRDT slots join the collected deltas, its account fields
    /// and plain slots go through the overlay.
    fn import(&mut self, id: ID, incoming: EvmState) -> Result<(), StoreError> {
        let native = self.crdt.merge_delta_from(id, incoming)?;
        self.state
            .overlay
            .apply(native, self.state.fallback.as_ref())
    }
}

impl<DB: DatabaseRef + Sync, Store: OverlayWrite> Committer<ID, EvmState>
    for EvmStateCommitter<DB, Store>
{
    /// Holds a transaction's state until its verdict is known.
    fn buffer(&mut self, id: ID, state: EvmState) {
        self.buffered.insert(Self::key(id), (id, state));
    }

    /// Imports the buffered states of the accepted transactions and drops the rest.
    fn stage(&mut self, accepted: Vec<ID>) -> Result<(), StoreError> {
        let accepted = accepted.into_iter().map(Self::key).collect::<HashSet<_>>();
        for (key, (id, incoming)) in std::mem::take(&mut self.buffered) {
            if accepted.contains(&key) {
                self.import(id, incoming)?;
            }
        }
        Ok(())
    }

    /// Imports any states still buffered, then applies the collected CRDT changes once.
    fn flush(&mut self) -> Result<(), StoreError> {
        for (id, incoming) in std::mem::take(&mut self.buffered).into_values() {
            self.import(id, incoming)?;
        }
        self.crdt.apply(&mut self.state)
    }
}

/// Applies `crdt`'s deltas into `db` and clears them: every owner's writes are computed in
/// parallel against the store, read-only, then written in one batch.
pub(crate) fn apply_crdt<DB: DatabaseRef + Sync, Store: OverlayWrite>(
    db: &mut StateDb<DB, Store>,
    crdt: &mut CrdtSlot,
) -> Result<(), StoreError> {
    let fallback = db.fallback.as_ref();
    let store = db.overlay.inner_mut();
    let updates = {
        let base = StateDb::with_fallback(
            StateOverlay::new(&*store, StateOverlay::copy_balance_nonce),
            fallback,
        );
        // Phase 1: each owner writes into its own overlay over the store.
        let stores = crdt
            .drain()
            .into_par_iter()
            .map(|(owner, sets, scalars)| {
                let mut store = StateDb::with_fallback(
                    StateOverlay::new(EvmState::default(), StateOverlay::copy_balance_nonce),
                    Some(&base),
                );
                CrdtSlot::apply_owner(&mut store, owner, sets, scalars)?;
                Ok(store)
            })
            .collect::<Result<Vec<_>, StoreError>>()?;
        let mut updates = EvmState::default();
        for store in stores {
            for (address, mut account) in store.into_inner() {
                // Keep the stored account and the stored slots' original values; only mark the
                // account touched.
                if let Some(mut current) = base
                    .inner()
                    .account(address, |current| current.map(clone_without_storage))
                {
                    current.storage = std::mem::take(&mut account.storage);
                    current.mark_touch();
                    account = current;
                }
                for (&key, slot) in account.storage.iter_mut() {
                    if let Some(original) = base
                        .inner()
                        .slot(address, key, |kept| kept.map(|kept| kept.original_value))
                    {
                        slot.original_value = original;
                    }
                }
                updates.insert(address, account);
            }
        }
        updates
    };
    // Phase 2: write the updates in one batch.
    store.write_accounts(updates);
    Ok(())
}

#[cfg(test)]
#[path = "tests/committer.rs"]
mod tests;
