use super::{
    Checker,
    checkers::{access::AccessChecker, sum::SumChecker},
    commit_plan::ConflictResult,
};
use crate::crdt::state::{
    Tracked,
    status::{Status, Tag},
    value::{CrdtValue, ValueSnapshots},
};
use crate::eu::output::ExecutionOutput;
use alloy_primitives::{I256, U256};

/// Coordinates access and numeric conflict checks and combines their results.
pub struct ConflictDetector<K> {
    access_checker: AccessChecker<K>,
    signed_checker: SumChecker<K, I256>,
    unsigned_checker: SumChecker<K, U256>,
    summary: ConflictResult<K>, // Conflict summary for the current batch.
}

impl<K> ConflictDetector<K> {
    pub fn new() -> Self {
        Self {
            access_checker: AccessChecker::new(),
            signed_checker: SumChecker::new(),
            unsigned_checker: SumChecker::new(),
            summary: ConflictResult::default(),
        }
    }
}

impl<K: Clone + Ord + std::hash::Hash + Send + Sync> Checker<K> for ConflictDetector<K> {
    fn import(&mut self, mut executed_jobs: Vec<ExecutionOutput<K, Tracked<Status, Status>>>) {
        let mut i256_records = Vec::new();
        let mut u256_records = Vec::new();
        for result in &mut executed_jobs {
            self.summary
                .job_info
                .entry(result.id)
                .or_insert(result.gas_used);
            if !matches!(
                &result.data.value.current,
                Status::Value(CrdtValue::I256(_) | CrdtValue::U256(_))
            ) {
                continue;
            }

            let Status::Value(value) =
                std::mem::replace(&mut result.data.value.current, Status::Tag(Tag::Stripped))
            else {
                unreachable!();
            };

            match value {
                CrdtValue::I256(value) => i256_records.push(result.clone_with_data(
                    result.data.clone_with_states(ValueSnapshots {
                        original: result.data.value.original.clone(),
                        current: value.into_owned(),
                    }),
                )),
                CrdtValue::U256(value) => u256_records.push(result.clone_with_data(
                    result.data.clone_with_states(ValueSnapshots {
                        original: result.data.value.original.clone(),
                        current: value.into_owned(),
                    }),
                )),
                _ => unreachable!(),
            }
        }
        self.access_checker.import(executed_jobs);
        self.signed_checker.import(i256_records);
        self.unsigned_checker.import(u256_records);
    }

    /// Checks all records accumulated by imports, consuming the current batch.
    fn check(&mut self) -> ConflictResult<K> {
        let (
            (mut rejected, mut pairs),
            ((signed_checker, signed_pairs), (unsigned_checker, unsigned_pairs)),
        ) = rayon::join(
            || self.access_checker.check(),
            || {
                rayon::join(
                    || self.signed_checker.check_bounds(),
                    || self.unsigned_checker.check_bounds(),
                )
            },
        );
        pairs.extend(signed_pairs);
        pairs.extend(unsigned_pairs);
        rejected.extend(signed_checker);
        rejected.extend(unsigned_checker);
        rejected.sort_unstable_by_key(|(record, _)| record.id.global);
        self.summary.violations = rejected;
        self.summary.pairs = pairs;

        // Calculate prefix gas in scheduled job order, counting each job once.
        let mut jobs = self.summary.job_info.iter_mut().collect::<Vec<_>>();
        jobs.sort_unstable_by_key(|(id, _)| (id.generation, id.sequence, id.job));
        for (id, gas) in jobs {
            if !self
                .summary
                .sequence_gas
                .last()
                .and_then(|sequence| sequence.last())
                .is_some_and(|(previous, _)| {
                    (previous.generation, previous.sequence) == (id.generation, id.sequence)
                })
            {
                self.summary.sequence_gas.push(Vec::new());
            }
            let cumulative = self.summary.sequence_gas.last_mut().unwrap();
            *gas += cumulative.last().map_or(0, |(_, gas)| *gas);
            cumulative.push((*id, *gas));
        }
        for (record, _) in &mut self.summary.violations {
            record.accumulated_gas = self.summary.job_info[&record.id];
        }
        self.summary.sequence_gas.sort_unstable_by(|left, right| {
            let (left, left_gas) = left.last().unwrap();
            let (right, right_gas) = right.last().unwrap();
            left.generation
                .cmp(&right.generation)
                .then_with(|| right_gas.cmp(left_gas))
                .then_with(|| left.sequence.cmp(&right.sequence))
        });
        std::mem::take(&mut self.summary)
    }
}
