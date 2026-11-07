use std::collections::{BTreeMap, HashSet};

use alloy_evm::revm::{DatabaseCommit, state::EvmState};
use rayon::prelude::*;

use super::StateDb;
use crate::execution::workload::ID;
use crate::store::traits::Committer;

/// Buffers transaction states and commits accepted changes in transaction-ID order.
#[derive(Debug, Default)]
pub struct EvmStateCommitter {
    state: StateDb,
    buffered: BTreeMap<ID, StateDb>,
}

impl EvmStateCommitter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_target(target: EvmState) -> Self {
        Self {
            state: StateDb::new(target),
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

impl Committer<ID, EvmState> for EvmStateCommitter {
    fn buffer(&mut self, id: ID, state: EvmState) {
        self.buffered.insert(id, StateDb::new(state));
    }

    fn stage(&mut self, accepted: Vec<ID>) {
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
