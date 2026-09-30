use super::traits::Rejected;
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
}

impl<K> Default for ConflictResult<K> {
    fn default() -> Self {
        Self {
            committable: BTreeSet::new(),
            rejected: Vec::new(),
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
