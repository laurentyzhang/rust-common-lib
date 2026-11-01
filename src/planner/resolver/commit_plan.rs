use super::Rejected;
use crate::crdt::state::{Status, Tracked};
use crate::eu::output::ExecutionOutput;
use crate::planner::scheduler::ID;
use alloy_evm::revm::state::EvmState;
use std::collections::{BTreeMap, BTreeSet};

pub struct CommitPlan {
    pub block_id: u64,
    pub generation: u64,
    pub accepted: Vec<u64>,
}

pub struct RejectPlan<K> {
    pub block_id: u64,
    pub generation: u64,
    pub rejected: Vec<u64>,
    pub violations: Vec<Rejected<K>>,
}

pub struct ConflictResult<K> {
    /// One job per (generation, sequence, job), with its ID and accumulated gas.
    pub transactions: BTreeMap<(u64, u64, u64), (ID, u64)>,
    /// Ordered sequences with cumulative gas for each job.
    pub sequence_gas: Vec<(ID, Vec<u64>)>,
    /// Numeric records retained for validating a different sequence selection.
    pub numeric: Vec<ExecutionOutput<K, Tracked<Status, Status>>>,
    pub committable: BTreeSet<u64>,
    /// Constraint violations reported before sequence selection.
    pub violations: Vec<Rejected<K>>,
    /// Transaction pairs that caused access or aggregate numeric conflicts.
    pub conflicts: BTreeSet<(ID, ID)>,
    /// Conflict-history storage updates to merge with the block's execution state.
    pub history: EvmState,
}

impl<K> Default for ConflictResult<K> {
    fn default() -> Self {
        Self {
            transactions: BTreeMap::new(),
            sequence_gas: Vec::new(),
            numeric: Vec::new(),
            committable: BTreeSet::new(),
            violations: Vec::new(),
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
            rejected: self
                .transactions
                .values()
                .filter(|(id, _)| !self.committable.contains(&id.global))
                .map(|(id, _)| id.global)
                .collect(),
            violations: self.violations,
        }
    }
}
