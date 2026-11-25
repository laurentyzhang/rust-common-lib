use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::{DatabaseRef, database_interface::EmptyDB, state::EvmState};
use std::io::Error;

use super::{
    StateDb,
    overlay::{OverlayState, StateOverlay},
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

impl<DB: DatabaseRef, Store: OverlayState> EvmStateCommitter<DB, Store> {
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
        let native = self.crdt.merge_delta(id, incoming)?;
        self.state
            .overlay
            .apply(native, self.state.fallback.as_ref())
    }
}

impl<DB: DatabaseRef, Store: OverlayState> Committer<ID, EvmState>
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

#[cfg(test)]
#[path = "tests/committer.rs"]
mod tests;
