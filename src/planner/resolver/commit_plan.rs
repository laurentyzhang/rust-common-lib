use super::Violation;
use crate::planner::scheduler::{ID, ScheduleKey};
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
    /// Accumulated gas keyed by scheduled job order.
    pub gas_lookup: BTreeMap<ScheduleKey, u64>,
    /// Ordered sequences of job IDs paired with cumulative gas.
    pub cumulative_gas: Vec<Vec<(ID, u64)>>,

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
            cumulative_gas: Vec::new(),
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
        let mut rejected = self
            .gas_lookup
            .keys()
            .map(|key| key.0)
            .filter(|id| !self.committable.contains(id))
            .map(|id| id.global)
            .collect::<Vec<_>>();
        rejected.sort_unstable();
        RejectPlan {
            block_id,
            generation,
            rejected,
            violations: self.violations,
        }
    }
}
