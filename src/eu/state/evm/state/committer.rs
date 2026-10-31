use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::{DatabaseCommit, state::EvmState};
use rayon::prelude::*;

use super::StateDb;
use crate::store::traits::Committer;

/// Buffers transaction states and commits accepted changes in transaction-ID order.
#[derive(Debug, Default)]
pub struct StatDbCommitter {
    state: StateDb,
    buffered: BTreeMap<u64, StateDb>,
}

impl StatDbCommitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_state(state: EvmState) -> Self {
        Self {
            state: StateDb::new(state),
            buffered: BTreeMap::new(),
        }
    }

    pub fn inner(&self) -> &EvmState {
        self.state.inner()
    }

    pub fn into_inner(self) -> EvmState {
        self.state.into_inner()
    }
}

impl Committer<u64, EvmState> for StatDbCommitter {
    fn buffer(&mut self, id: u64, state: EvmState) {
        self.buffered.insert(id, StateDb::new(state));
    }

    fn stage(&mut self, accepted: Vec<u64>) {
        let accepted = accepted.into_iter().collect::<HashSet<_>>();
        self.buffered.retain(|id, _| accepted.contains(id));
    }

    fn flush(&mut self) {
        let mut states: Vec<_> = std::mem::take(&mut self.buffered).into_values().collect();

        // Combine adjacent accepted states without changing transaction order.
        while states.len() > 1 {
            states.par_chunks_mut(2).for_each(|pair| {
                if let [left, right] = pair {
                    left.merge(std::mem::take(right));
                }
            });
            let remaining = states.len().div_ceil(2);
            for index in 1..remaining {
                states.swap(index, index * 2);
            }
            states.truncate(remaining);
        }

        if let Some(state) = states.pop() {
            let mut merged = std::mem::take(&mut self.state);
            merged.merge(state);
            self.state.commit(merged.into_inner());
        }
    }
}

#[cfg(test)]
#[path = "tests/committer.rs"]
mod tests;
