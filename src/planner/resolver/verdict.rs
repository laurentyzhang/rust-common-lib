use super::Reason;
use crate::planner::scheduler::{ID, ScheduleKey};
use alloy_evm::revm::state::EvmState;
use std::collections::{BTreeMap, BTreeSet};

/// Reserved metadata ID template; checked jobs supply its block and generation.
pub const CONFLICT_META_ID: ID = ID::new(0, u64::MAX);

/// The final commit decision for one job; no rejection means accepted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitVerdict<K> {
    pub id: ID,
    pub rejection: Option<Rejection<K>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection<K> {
    Conflict {
        /// Conflicting jobs, or contributors to a numeric bound failure.
        peers: Vec<ID>,
        reasons: Vec<(K, Reason)>,
    },
    /// An earlier rejected job in the same sequence.
    Dependency(ID),
}

pub fn accepted_ids<K>(verdicts: &[CommitVerdict<K>]) -> Vec<ID> {
    verdicts
        .iter()
        .filter(|verdict| verdict.rejection.is_none())
        .map(|verdict| verdict.id)
        .collect()
}

pub fn rejected_ids<K>(verdicts: &[CommitVerdict<K>]) -> Vec<ID> {
    verdicts
        .iter()
        .filter(|verdict| verdict.rejection.is_some())
        .map(|verdict| verdict.id)
        .collect()
}

pub struct ConflictResult<K> {
    /// Accumulated gas keyed by scheduled job order.
    pub gas_lookup: BTreeMap<ScheduleKey, u64>,
    /// Ordered sequences of job IDs paired with cumulative gas.
    pub cumulative_gas: Vec<Vec<(ID, u64)>>,

    /// One final decision per job, ordered by ID.
    pub verdicts: Vec<CommitVerdict<K>>,
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
            verdicts: Vec::new(),
            involved_pairs: BTreeSet::new(),
            conflict_meta: (CONFLICT_META_ID, EvmState::default()),
        }
    }
}
