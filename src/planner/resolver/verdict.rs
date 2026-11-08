use super::Reason;
use crate::planner::scheduler::ID;

/// A job's commit decision and complete diagnostics, including dependency rejections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitVerdict<K> {
    pub id: ID,
    pub accepted: bool,
    /// All conflicting jobs, including when this job is accepted or rejected by dependency.
    pub peers: Vec<ID>,
    /// Dependency reasons have no key; access and numeric reasons retain their keys.
    pub reasons: Vec<(Option<K>, Reason)>,
}

pub fn accepted_ids<'a, K: 'a>(
    verdicts: impl IntoIterator<Item = &'a CommitVerdict<K>>,
) -> Vec<ID> {
    verdicts
        .into_iter()
        .filter(|verdict| verdict.accepted)
        .map(|verdict| verdict.id)
        .collect()
}

pub fn rejected_ids<'a, K: 'a>(
    verdicts: impl IntoIterator<Item = &'a CommitVerdict<K>>,
) -> Vec<ID> {
    verdicts
        .into_iter()
        .filter(|verdict| !verdict.accepted)
        .map(|verdict| verdict.id)
        .collect()
}
