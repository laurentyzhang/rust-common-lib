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
mod tests {
    use super::*;

    #[test]
    fn pending_is_neither_accepted_nor_rejected() {
        let verdicts = [
            CommitVerdict::<()> {
                id: ID::new(0, 0),
                decision: CommitDecision::Pending,
                conflicts: Vec::new(),
            },
            CommitVerdict {
                id: ID::new(0, 1),
                decision: CommitDecision::Accepted,
                conflicts: Vec::new(),
            },
            CommitVerdict {
                id: ID::new(0, 2),
                decision: CommitDecision::Rejected,
                conflicts: Vec::new(),
            },
        ];
        assert_eq!(accepted_ids(&verdicts), vec![verdicts[1].id]);
        assert_eq!(rejected_ids(&verdicts), vec![verdicts[2].id]);
    }
}
