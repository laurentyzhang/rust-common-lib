use super::Violation;
use crate::planner::scheduler::ID;
use alloy_evm::revm::state::EvmState;
use std::collections::{BTreeMap, BTreeSet};

/// Reserved job ID for discovered conflict metadata.
pub const CONFLICT_META_ID: ID = ID::new(u64::MAX);

pub struct CommitPlan {
    pub block_id: u64,
    pub generation: u64,
    pub accepted: Vec<u64>,
}

pub struct RejectPlan<K> {
    pub block_id: u64,
    pub generation: u64,
    pub rejected: Vec<u64>,
    pub violations: Vec<Violation<K>>,
}

pub struct ConflictResult<K> {
    /// Each job's ID mapped to its accumulated gas.
    pub gas_lookup: BTreeMap<ID, u64>,
    /// Ordered sequences of job IDs paired with cumulative gas.
    pub sequence_gas: Vec<Vec<(ID, u64)>>,

    /// Committable job IDs.
    pub committable: BTreeSet<ID>,
    /// Constraint violations reported before sequence selection.
    pub violations: Vec<Violation<K>>,
    /// Job pairs involved in access or aggregate numeric violations.
    pub involved_pairs: BTreeSet<(ID, ID)>,
    /// Conflict metadata job ID and storage updates.
    pub conflict_meta: (ID, EvmState),
}

impl<K> Default for ConflictResult<K> {
    fn default() -> Self {
        Self {
            gas_lookup: BTreeMap::new(),
            sequence_gas: Vec::new(),
            committable: BTreeSet::new(),
            violations: Vec::new(),
            involved_pairs: BTreeSet::new(),
            conflict_meta: (CONFLICT_META_ID, EvmState::default()),
        }
    }
}

impl<K> ConflictResult<K> {
    pub fn commit_plan(&self, block_id: u64, generation: u64) -> CommitPlan {
        CommitPlan {
            block_id,
            generation,
            accepted: self.committable.iter().map(|id| id.global).collect(),
        }
    }

    pub fn reject_plan(self, block_id: u64, generation: u64) -> RejectPlan<K> {
        RejectPlan {
            block_id,
            generation,
            rejected: self
                .gas_lookup
                .keys()
                .filter(|id| !self.committable.contains(*id))
                .map(|id| id.global)
                .collect(),
            violations: self.violations,
        }
    }
}
