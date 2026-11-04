use super::{
    Checker, Reason,
    checkers::{access::AccessChecker, sum::SumChecker},
    commit_plan::ConflictResult,
    selection,
};
use crate::crdt::state::{
    Tracked,
    status::Status,
    value::{CrdtValue, ValueSnapshots},
};
use crate::eu::output::ExecutionOutput;
use crate::planner::scheduler::{ID, ScheduleKey};
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

    fn accumulate(&mut self) -> Vec<Vec<(ID, u64)>> {
        // Group jobs by their scheduled sequence.
        let mut sequences = Vec::<Vec<(ID, u64)>>::new();
        let mut previous = None;
        for (key, gas) in &self.summary.gas_lookup {
            let id = key.0;
            let sequence = (id.generation, id.sequence);
            if previous != Some(sequence) {
                sequences.push(Vec::new());
                previous = Some(sequence);
            }
            sequences.last_mut().unwrap().push((id, *gas));
        }

        // Accumulate each sequence's gas and update the lookup in the same order.
        let mut gas_values = self.summary.gas_lookup.values_mut();
        for sequence in &mut sequences {
            let mut cumulative = 0;
            for ((_, gas), lookup_gas) in sequence.iter_mut().zip(&mut gas_values) {
                cumulative += *gas;
                *gas = cumulative;
                *lookup_gas = cumulative;
            }
        }
        sequences
    }
}

impl<K: Clone + Ord + std::hash::Hash + Send + Sync> Checker<K> for ConflictDetector<K> {
    fn import(&mut self, executed_jobs: Vec<ExecutionOutput<K, Tracked<Status, Status>>>) {
        let mut i256_records = Vec::new();
        let mut u256_records = Vec::new();
        for job in &executed_jobs {
            self.summary
                .gas_lookup
                .entry(ScheduleKey(job.id))
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
        let ((mut rejected, pairs), ((i256, i256_pairs), (u256, u256_pairs))) = rayon::join(
            || self.access_checker.check(),
            || {
                rayon::join(
                    || self.i256_checker.check_bounds(),
                    || self.u256_checker.check_bounds(),
                )
            },
        );

        rejected.extend(i256.into_iter().chain(u256));
        self.summary.violations = rejected;
        self.summary
            .violations
            .sort_unstable_by_key(|(record, _)| record.id.global);
        self.summary.involved_pairs = pairs
            .iter()
            .copied()
            .chain(i256_pairs)
            .chain(u256_pairs)
            .collect();

        self.summary.cumulative_gas = self.accumulate();

        for (record, _) in &mut self.summary.violations {
            record.accumulated_gas = self.summary.gas_lookup[&ScheduleKey(record.id)];
        }

        self.summary.committable = selection::select(
            &self.summary.cumulative_gas,
            &pairs,
            self.summary
                .violations
                .iter()
                .filter_map(|(record, reason)| {
                    matches!(reason, Reason::NumericError(_)).then_some(record.id)
                }),
        );
        std::mem::take(&mut self.summary)
    }
}
