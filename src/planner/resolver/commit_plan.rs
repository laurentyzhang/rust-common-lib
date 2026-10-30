use super::traits::Rejected;
use alloy_evm::revm::state::EvmState;
use std::collections::BTreeSet;

pub struct CommitPlan {
    pub block_id: u64,
    pub generation: u64,
    pub accepted: Vec<u64>,
}

pub struct RejectPlan<K> {
    pub block_id: u64,
    pub generation: u64,
    pub rejected: Vec<Rejected<K>>,
}

pub struct ConflictResult<K> {
    pub committable: BTreeSet<u64>,
    pub rejected: Vec<Rejected<K>>,
    /// Transaction pairs that caused access or aggregate numeric conflicts.
    pub conflicts: BTreeSet<(u64, u64)>,
    /// Conflict-history storage updates to merge with the block's execution state.
    pub history: EvmState,
}

impl<K> Default for ConflictResult<K> {
    fn default() -> Self {
        Self {
            committable: BTreeSet::new(),
            rejected: Vec::new(),
            conflicts: BTreeSet::new(),
            history: EvmState::default(),
        }
    }
}

impl<K> ConflictResult<K> {
    pub fn commit_plan(&self, block_id: u64, generation: u64) -> CommitPlan {
        CommitPlan {
            block_id,
            generation,
            accepted: self.committable.iter().copied().collect(),
        }
    }

    pub fn reject_plan(self, block_id: u64, generation: u64) -> RejectPlan<K> {
        RejectPlan {
            block_id,
            generation,
            rejected: self.rejected,
        }
    }
}
