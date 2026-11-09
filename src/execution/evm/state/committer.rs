use std::collections::{HashMap, HashSet};

use alloy_evm::revm::{DatabaseRef, state::EvmState};
use std::io::Error;

use super::{StateDb, overlay::StateOverlay};
use crate::execution::workload::ID;
use crate::store::traits::Committer;

/// Buffers transaction states and commits accepted changes.
#[derive(Debug, Default)]
pub struct EvmStateCommitter {
    state: StateDb,
    buffered: HashMap<ID, EvmState>,
}

impl EvmStateCommitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_target(target: EvmState) -> Self {
        Self {
            state: StateDb::new(target),
            buffered: HashMap::new(),
        }
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
                .merge(incoming, Some(fallback))
                .map_err(Error::other)?;
        }
        Ok(overlay.into_inner())
    }
}

impl Committer<ID, EvmState> for EvmStateCommitter {
    fn buffer(&mut self, id: ID, state: EvmState) {
        self.buffered.insert(id, state);
    }

    fn stage(&mut self, accepted: Vec<ID>) {
        let accepted = accepted.into_iter().collect::<HashSet<_>>();
        self.buffered.retain(|id, _| accepted.contains(id));
    }

    fn flush(&mut self) {
        for incoming in std::mem::take(&mut self.buffered).into_values() {
            self.state
                .overlay
                .merge(incoming, self.state.fallback.as_ref())
                .unwrap();
        }
    }
}

#[cfg(test)]
#[path = "tests/committer.rs"]
mod tests;
