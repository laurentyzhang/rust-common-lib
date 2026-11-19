use crate::crdt::{Scalar, Type};
use crate::{
    crdt::{
        Crdt,
        state::{Status, Tracked},
    },
    execution::output::ExecutionOutput,
    planner::resolver::checkers::util::run_parallel,
    planner::resolver::{Diagnostic, Reason},
    planner::scheduler::{ID, ScheduleKey},
};
use std::collections::{BTreeSet, HashMap};

pub struct SumChecker<K, T: Type> {
    pub(in crate::planner::resolver) entries:
        BTreeSet<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>,
    counts: HashMap<K, usize>,
}

impl<K, T: Type> SumChecker<K, T> {
    /// Creates an empty numeric bounds checker.
    pub fn new() -> Self {
        Self {
            entries: BTreeSet::new(),
            counts: HashMap::new(),
        }
    }

    /// Collects numeric snapshots, retaining each job's record for cutoff selection.
    pub fn import(&mut self, records: Vec<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>)
    where
        K: Clone + Ord + std::hash::Hash,
    {
        for record in records {
            *self.counts.entry(record.key.clone()).or_default() += 1;
            self.entries.insert(record);
        }
    }

    /// Removes diagnosed jobs, then consumes and checks the remaining records.
    pub fn check_bounds(&mut self, diagnostics: Vec<Diagnostic<K>>) -> Vec<Diagnostic<K>>
    where
        K: Ord + std::hash::Hash + Send + Sync,
        T: Send,
        T::Magnitude: Send,
    {
        self.entries.retain(|record| {
            if !diagnostics
                .iter()
                .any(|diagnostic| diagnostic.id == record.id)
            {
                return true;
            }
            let count = self.counts.get_mut(&record.key).unwrap();
            *count -= 1;
            if *count == 0 {
                self.counts.remove(&record.key);
            }
            false
        });
        run_parallel(&mut self.entries, &mut self.counts, Self::do_check)
    }

    /// Orders one key's records by sequence/job, then checks negative and positive
    /// changes independently from the same initial value.
    fn do_check(
        mut records: Vec<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>,
    ) -> Vec<Diagnostic<K>>
    where
        K: Ord,
    {
        let mut diagnostics = Vec::new();
        records.sort_unstable_by_key(|record| ScheduleKey(record.id));
        let Some(first) = records.first() else {
            return diagnostics;
        };
        let mut initial = first.data.current().clone();
        initial.delta = None;

        Self::check_sum_bounds(
            &mut records,
            initial.clone(),
            |before, after| after < before,
            &mut diagnostics,
        );
        Self::check_sum_bounds(
            &mut records,
            initial,
            |before, after| after > before,
            &mut diagnostics,
        );
        diagnostics
    }

    /// Accumulates job changes accepted by `select(before, after)`.
    /// Cuts each failing job's sequence suffix and records its diagnostic and peers.
    fn check_sum_bounds(
        records: &mut Vec<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>,
        mut summed: Scalar<T>,
        select: impl Fn(T, T) -> bool,
        diagnostics: &mut Vec<Diagnostic<K>>,
    ) {
        let mut previous_sequence = None;
        let mut previous_value = summed.value;
        let mut contributors = Vec::<ID>::new();
        let mut index = 0;

        while index < records.len() {
            let record = &records[index];
            let sequence = (record.id.block, record.id.generation, record.id.sequence);
            let scalar = record.data.current();
            if previous_sequence != Some(sequence) {
                previous_sequence = Some(sequence);
                previous_value = scalar.value;
            }

            // Consecutive snapshots give this job's change, including a return to zero.
            let before = previous_value;
            previous_value = scalar.clone().apply_delta().value;
            if !select(before, previous_value) {
                index += 1;
                continue;
            }
            let delta = T::delta_between(before, previous_value);

            if let Err(error) = summed.add_delta(&delta) {
                let failed = records.remove(index);
                diagnostics.push(Diagnostic {
                    id: failed.id,
                    peers: contributors.clone(),
                    reason: Reason::NumericError(failed.key, error.into()),
                });

                // Drop this sequence's suffix and continue with the next sequence.
                records.retain(|record| {
                    (record.id.block, record.id.generation, record.id.sequence) != sequence
                        || record.id.job < failed.id.job
                });
                continue;
            }
            contributors.push(record.id);
            index += 1;
        }
    }
}
