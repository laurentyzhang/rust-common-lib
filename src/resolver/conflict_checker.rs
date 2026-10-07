use super::{
    access_checker::AccessChecker,
    commit_plan::ConflictResult,
    sum_checker::SumChecker,
    traits::{Checker, Rejected},
};
use crate::crdt::state::{
    Tracked,
    status::{Status, Tag},
    value::{Value, Values},
};
use crate::execution::output::ExecutionOutput;
use std::collections::{BTreeSet, HashMap};

/// Coordinates access and numeric conflict checks and combines their results.
pub struct ConflictChecker<K> {
    access_checker: AccessChecker<K>,
    sum_checker: SumChecker<K>,
    result: ConflictResult<K>,
}

impl<K> ConflictChecker<K> {
    pub fn new() -> Self {
        Self {
            access_checker: AccessChecker::new(),
            sum_checker: SumChecker::new(),
            result: ConflictResult::default(),
        }
    }

    pub fn import(&mut self, mut transitions: Vec<ExecutionOutput<K, Tracked<Status, Status>>>)
    where
        K: Clone + Eq + std::hash::Hash,
    {
        for transaction_id in transitions.iter().map(|record| record.id) {
            self.result.committable.insert(transaction_id);
        }

        let mut numeric_trans = HashMap::new();
        for transition in &mut transitions {
            if matches!(
                &transition.data.value.current,
                Status::Value(Value::Numeric(_))
            ) {
                let Status::Value(Value::Numeric(value)) = std::mem::replace(
                    &mut transition.data.value.current,
                    Status::Tag(Tag::Stripped),
                ) else {
                    unreachable!();
                };

                numeric_trans
                    .entry(transition.key.clone())
                    .or_insert_with(BTreeSet::new)
                    .insert(
                        transition.clone_with_data(transition.data.clone_with_states(Values {
                            original: transition.data.value.original.clone(),
                            current: value,
                        })),
                    );
            }
        }

        self.access_checker.import(transitions);
        self.sum_checker.import(numeric_trans);
    }

    /// Runs both stages in parallel and retains every rejection reason,
    /// including multiple reasons for the same transaction and key.
    pub fn check_conflicts(&mut self) -> ConflictResult<K>
    where
        K: Eq + std::hash::Hash + Send + Sync,
    {
        let (mut rejected, numeric_rejected) = rayon::join(
            || self.access_checker.check_conflicts(),
            || self.sum_checker.accumulate(),
        );
        rejected.extend(numeric_rejected);
        rejected.sort_unstable_by_key(|(record, _)| record.id);
        let rejected_ids = rejected
            .iter()
            .map(|(record, _)| record.id)
            .collect::<BTreeSet<_>>();
        self.result
            .committable
            .retain(|transaction_id| !rejected_ids.contains(transaction_id));
        self.result.rejected = rejected;
        std::mem::take(&mut self.result)
    }
}

/// Checks access conflicts in already-grouped records.
pub fn detect<K>(
    records_by_key: &mut HashMap<K, BTreeSet<ExecutionOutput<K, Tracked<Status, Status>>>>,
) -> Vec<Rejected<K>>
where
    K: Eq + std::hash::Hash + Send + Sync,
{
    AccessChecker::<K>::find_rejections(records_by_key)
}
