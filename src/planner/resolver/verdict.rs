use super::Reason;
use crate::planner::scheduler::ID;

/// Whether the checker has accepted or rejected a job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CommitDecision {
    #[default]
    Pending,
    Accepted,
    Rejected,
    Dropped, // The job was dropped, no state changes applied, not even nonce or fee updates.
}

/// A job's commit decision and complete diagnostics, including dependency rejections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitVerdict<K> {
    pub id: ID,
    pub decision: CommitDecision,
    pub conflicts: Vec<(ID, Reason<K>)>, // Related job and the reason.
}

pub fn accepted_ids<'a, K: 'a>(
    verdicts: impl IntoIterator<Item = &'a CommitVerdict<K>>,
) -> Vec<ID> {
    verdicts
        .into_iter()
        .filter(|verdict| verdict.decision == CommitDecision::Accepted)
        .map(|verdict| verdict.id)
        .collect()
}

pub fn rejected_ids<'a, K: 'a>(
    verdicts: impl IntoIterator<Item = &'a CommitVerdict<K>>,
) -> Vec<ID> {
    verdicts
        .into_iter()
        .filter(|verdict| verdict.decision == CommitDecision::Rejected)
        .map(|verdict| verdict.id)
        .collect()
}

#[cfg(test)]
#[path = "tests/verdict.rs"]
mod tests;
