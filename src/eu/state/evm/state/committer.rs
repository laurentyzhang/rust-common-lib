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
mod tests {
    use super::*;
    use crate::eu::{ApiAddress, Field, KeyCodec};
    use alloy_evm::revm::state::{Account, EvmStorageSlot};
    use alloy_primitives::{Address, U256};

    #[test]
    fn flush_combines_only_accepted_deltas_and_clears_the_buffer() {
        let key = KeyCodec::encode(ApiAddress::Scalar, Field::PosDelta as u8, Address::ZERO);
        let changes = |original: u64, present: u64| {
            let mut account = Account::default();
            account.mark_touch();
            account.storage.insert(
                key,
                EvmStorageSlot::new_changed(
                    U256::from(original),
                    U256::from(present),
                    Default::default(),
                ),
            );
            EvmState::from_iter([(Address::ZERO, account)])
        };
        let mut committer = StatDbCommitter::with_state(changes(0, 10));
        committer.buffer(2, changes(10, 13));
        committer.buffer(1, changes(10, 12));
        committer.buffer(3, changes(10, 99));
        committer.stage(vec![1, 2]);
        committer.flush();
        assert_eq!(
            committer.inner()[&Address::ZERO].storage[&key].present_value(),
            U256::from(15)
        );
        committer.flush();
        assert_eq!(
            committer.inner()[&Address::ZERO].storage[&key].present_value(),
            U256::from(15)
        );
        committer.buffer(4, changes(15, 16));
        committer.flush();
        assert_eq!(
            committer.into_inner()[&Address::ZERO].storage[&key].present_value(),
            U256::from(16)
        );
    }
}
