use super::{
    access_checker::AccessChecker,
    commit_plan::ConflictResult,
    sum_checker::SumChecker,
    traits::{Checker, Rejected},
};
use crate::crdt::state::{
    Tracked,
    status::{Status, Tag},
    value::{CrdtValue, ValueSnapshots},
};
use crate::eu::output::ExecutionOutput;
use alloy_primitives::{I256, U256};
use std::collections::{BTreeSet, HashMap};

/// Coordinates access and numeric conflict checks and combines their results.
pub struct ConflictChecker<K> {
    access_checker: AccessChecker<K>,
    signed: SumChecker<K, I256>,
    unsigned: SumChecker<K, U256>,
    result: ConflictResult<K>,
}

impl<K> ConflictChecker<K> {
    pub fn new() -> Self {
        Self {
            access_checker: AccessChecker::new(),
            signed: SumChecker::new(),
            unsigned: SumChecker::new(),
            result: ConflictResult::default(),
        }
    }

    pub fn import(&mut self, mut transitions: Vec<ExecutionOutput<K, Tracked<Status, Status>>>)
    where
        K: Clone + Eq + std::hash::Hash,
    {
        for transaction_id in transitions.iter().map(|record| record.id.global) {
            self.result.committable.insert(transaction_id);
        }

        let mut signed = Vec::new();
        let mut unsigned = Vec::new();
        for transition in &mut transitions {
            if !matches!(
                &transition.data.value.current,
                Status::Value(CrdtValue::I256(_) | CrdtValue::U256(_))
            ) {
                continue;
            }
            let Status::Value(value) = std::mem::replace(
                &mut transition.data.value.current,
                Status::Tag(Tag::Stripped),
            ) else {
                unreachable!();
            };

            match value {
                CrdtValue::I256(value) => signed.push(transition.clone_with_data(
                    transition.data.clone_with_states(ValueSnapshots {
                        original: transition.data.value.original.clone(),
                        current: value.into_owned(),
                    }),
                )),
                CrdtValue::U256(value) => unsigned.push(transition.clone_with_data(
                    transition.data.clone_with_states(ValueSnapshots {
                        original: transition.data.value.original.clone(),
                        current: value.into_owned(),
                    }),
                )),
                _ => unreachable!(),
            }
        }
        self.access_checker.import(transitions);
        self.signed.import(signed);
        self.unsigned.import(unsigned);
    }

    /// Runs both stages in parallel and retains every rejection reason,
    /// including multiple reasons for the same transaction and key.
    pub fn check_conflicts(&mut self) -> ConflictResult<K>
    where
        K: Eq + std::hash::Hash + Send + Sync,
    {
        let ((mut rejected, mut conflicts), ((signed, signed_pairs), (unsigned, unsigned_pairs))) =
            rayon::join(
                || self.access_checker.check_conflicts(),
                || {
                    rayon::join(
                        || self.signed.check_bounds(),
                        || self.unsigned.check_bounds(),
                    )
                },
            );
        conflicts.extend(signed_pairs);
        conflicts.extend(unsigned_pairs);
        rejected.extend(signed);
        rejected.extend(unsigned);
        rejected.sort_unstable_by_key(|(record, _)| record.id.global);
        let rejected_ids = rejected
            .iter()
            .map(|(record, _)| record.id.global)
            .collect::<BTreeSet<_>>();
        self.result
            .committable
            .retain(|transaction_id| !rejected_ids.contains(transaction_id));
        self.result.rejected = rejected;
        self.result.conflicts = conflicts;
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
