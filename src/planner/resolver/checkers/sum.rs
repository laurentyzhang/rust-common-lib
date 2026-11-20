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

    /// Checks one key's sequence prefixes in schedule order.
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

        Self::check_sum_bounds(records, initial, &mut diagnostics);
        diagnostics
    }

    /// Checks both bounds using each sequence's latest net contribution.
    fn check_sum_bounds(
        records: Vec<ExecutionOutput<K, Tracked<Status, Scalar<T>>>>,
        summed: Scalar<T>,
        diagnostics: &mut Vec<Diagnostic<K>>,
    ) {
        let mut lower = summed.clone();
        let mut upper = summed;
        let mut previous_sequence = None;
        let mut sequence_start = lower.value;
        let mut previous_value = sequence_start;
        let mut lower_contributors = Vec::<ID>::new();
        let mut upper_contributors = Vec::<ID>::new();
        let mut lower_start = 0;
        let mut upper_start = 0;
        let mut rejected = false;

        for record in records {
            let sequence = (record.id.block, record.id.generation, record.id.sequence);
            let scalar = record.data.current();
            if previous_sequence != Some(sequence) {
                previous_sequence = Some(sequence);
                sequence_start = scalar.value;
                previous_value = sequence_start;
                lower_start = lower_contributors.len();
                upper_start = upper_contributors.len();
                rejected = false;
            }
            if rejected {
                continue;
            }
            let value = scalar.clone().apply_delta().value;
            if value == previous_value {
                continue;
            }

            // Replace the sequence's previous contribution in both sums together.
            let mut next_lower = lower.clone();
            let mut next_upper = upper.clone();
            let lower_delta = T::delta_between(
                previous_value.min(sequence_start),
                value.min(sequence_start),
            );
            let upper_delta = T::delta_between(
                previous_value.max(sequence_start),
                value.max(sequence_start),
            );
            let failure = if let Err(error) = next_lower.add_delta(&lower_delta) {
                Some((error, &lower_contributors))
            } else if let Err(error) = next_upper.add_delta(&upper_delta) {
                Some((error, &upper_contributors))
            } else {
                None
            };
            if let Some((error, contributors)) = failure {
                diagnostics.push(Diagnostic {
                    id: record.id,
                    peers: contributors.clone(),
                    reason: Reason::NumericError(record.key, error.into()),
                });
                rejected = true;
                continue;
            }

            lower = next_lower;
            upper = next_upper;
            if value >= sequence_start {
                lower_contributors.truncate(lower_start);
            } else if value < previous_value {
                lower_contributors.push(record.id);
            }
            if value <= sequence_start {
                upper_contributors.truncate(upper_start);
            } else if value > previous_value {
                upper_contributors.push(record.id);
            }
            previous_value = value;
        }
    }
}
