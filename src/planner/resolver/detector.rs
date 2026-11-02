use super::{
    Checker,
    checkers::{access::AccessChecker, sum::SumChecker},
    commit_plan::ConflictResult,
};
use crate::crdt::state::{
    Tracked,
    status::Status,
    value::{CrdtValue, ValueSnapshots},
};
use crate::eu::output::ExecutionOutput;
use alloy_primitives::{I256, U256};

/// Coordinates access and numeric conflict checks and combines their results.
pub struct ConflictDetector<K> {
    access_checker: AccessChecker<K>,
    i256_checker: SumChecker<K, I256>,
    u256_checker: SumChecker<K, U256>,
    summary: ConflictResult<K>, // Conflict summary for the current batch.
}

impl<K> ConflictDetector<K> {
    pub fn new() -> Self {
        Self {
            access_checker: AccessChecker::new(),
            i256_checker: SumChecker::new(),
            u256_checker: SumChecker::new(),
            summary: ConflictResult::default(),
        }
    }
}

impl<K: Clone + Ord + std::hash::Hash + Send + Sync> Checker<K> for ConflictDetector<K> {
    fn import(&mut self, executed_jobs: Vec<ExecutionOutput<K, Tracked<Status, Status>>>) {
        let mut i256_records = Vec::new();
        let mut u256_records = Vec::new();
        for job in &executed_jobs {
            self.summary
                .gas_lookup
                .entry(job.id)
                .or_insert(job.gas_used); // Insert gas used for this job if not already present.

            match &job.data.value.current {
                Status::Value(CrdtValue::I256(value)) => {
                    i256_records.push(job.clone_with_data(job.data.clone_with_states(
                        ValueSnapshots::from_refs(&job.data.value.original, value.as_ref()),
                    )))
                }
                Status::Value(CrdtValue::U256(value)) => {
                    u256_records.push(job.clone_with_data(job.data.clone_with_states(
                        ValueSnapshots::from_refs(&job.data.value.original, value.as_ref()),
                    )))
                }
                _ => {}
            }
        }
        self.access_checker.import(executed_jobs);
        self.i256_checker.import(i256_records);
        self.u256_checker.import(u256_records);
    }

    /// Checks all records accumulated by imports, consuming the current batch.
    fn check(&mut self) -> ConflictResult<K> {
        let ((mut rejected, mut pairs), ((i256, i256_pairs), (u256, u256_pairs))) = rayon::join(
            || self.access_checker.check(),
            || {
                rayon::join(
                    || self.i256_checker.check_bounds(),
                    || self.u256_checker.check_bounds(),
                )
            },
        );

        pairs.extend(i256_pairs.into_iter().chain(u256_pairs));
        rejected.extend(i256.into_iter().chain(u256));
        rejected.sort_unstable_by_key(|(record, _)| record.id.global);
        self.summary.violations = rejected;
        self.summary.involved_pairs = pairs;

        // Calculate gas in scheduled job order, counting each job once.
        let mut jobs_gas = self.summary.gas_lookup.iter_mut().collect::<Vec<_>>();
        jobs_gas.sort_unstable_by_key(|(id, _)| (id.generation, id.sequence, id.job));
        for (id, gas) in jobs_gas {
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

            // Update the cumulative gas for the current sequence.
            let seq_mut = self.summary.sequence_gas.last_mut().unwrap();
            *gas += seq_mut.last().map_or(0, |(_, gas)| *gas);
            seq_mut.push((*id, *gas));
        }

        for (record, _) in &mut self.summary.violations {
            record.accumulated_gas = self.summary.gas_lookup[&record.id];
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
