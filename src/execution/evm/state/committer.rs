use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::{
    DatabaseRef,
    state::{Account, EvmState},
};
use std::io::Error;

use super::{StateDb, overlay::StateOverlay};
use crate::crdt::state::CrdtError;
use crate::execution::workload::ID;
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

    /// Merges the buffered states against the shared base.
    pub(crate) fn commit<DB: DatabaseRef>(self, fallback: &DB) -> Result<EvmState, Error> {
        let mut overlay = StateOverlay::new(self.state.into_inner());
        for incoming in self.buffered.into_values() {
            overlay
                .apply(incoming, Some(fallback), self.nonce_fee_handler)
                .map_err(|error| Error::other(format!("{error:?}")))?;
        }
        Ok(overlay.into_inner())
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

    fn flush(&mut self) -> Result<(), StoreError> {
        let mut overlay = StateOverlay::new(self.state.inner().clone());
        for incoming in self.buffered.values() {
            overlay.apply(
                incoming.clone(),
                self.state.fallback.as_ref(),
                self.nonce_fee_handler,
            )?;
        }
        self.state.overlay = overlay;
        self.buffered.clear();
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/committer.rs"]
mod tests;
