use super::{
    checkers::{access::AccessChecker, sum::SumChecker},
    commit_plan::ConflictResult,
    Checker,
};
use crate::crdt::state::{
    Tracked,
    status::{Status, Tag},
    value::{CrdtValue, ValueSnapshots},
};
use crate::eu::output::ExecutionOutput;
use alloy_primitives::{I256, U256};
use std::collections::BTreeSet;

/// Coordinates access and numeric conflict checks and combines their results.
pub struct ConflictChecker<K> {
    records: BTreeSet<ExecutionOutput<K, Tracked<Status, Status>>>,
    access_checker: AccessChecker<K>,
    signed_checker: SumChecker<K, I256>,
    unsigned_checker: SumChecker<K, U256>,
    result: ConflictResult<K>,
}

impl<K> ConflictChecker<K> {
    pub fn new() -> Self {
        Self {
            records: BTreeSet::new(),
            access_checker: AccessChecker::new(),
            signed_checker: SumChecker::new(),
            unsigned_checker: SumChecker::new(),
            result: ConflictResult::default(),
        }
    }
}

impl<K: Clone + Ord + std::hash::Hash + Send + Sync> Checker<K> for ConflictChecker<K> {
    fn import(&mut self, transitions: Vec<ExecutionOutput<K, Tracked<Status, Status>>>) {
        self.records.extend(transitions);
    }

    /// Checks all records accumulated by imports, consuming the current batch.
    fn check(&mut self) -> ConflictResult<K> {
        let mut transitions = std::mem::take(&mut self.records)
            .into_iter()
            .collect::<Vec<_>>();
        for record in &transitions {
            self.result
                .transactions
                .entry((record.id.generation, record.id.sequence, record.id.job))
                .or_insert((record.id, record.gas_used));
        }
        let mut signed_checker = Vec::new();
        let mut unsigned_checker = Vec::new();
        for transition in &mut transitions {
            if !matches!(
                &transition.data.value.current,
                Status::Value(CrdtValue::I256(_) | CrdtValue::U256(_))
            ) {
                continue;
            }
            self.result.numeric.push(
                transition.clone_with_data(
                    transition
                        .data
                        .clone_with_states(transition.data.value.clone()),
                ),
            );
            let Status::Value(value) = std::mem::replace(
                &mut transition.data.value.current,
                Status::Tag(Tag::Stripped),
            ) else {
                unreachable!();
            };

            match value {
                CrdtValue::I256(value) => signed_checker.push(transition.clone_with_data(
                    transition.data.clone_with_states(ValueSnapshots {
                        original: transition.data.value.original.clone(),
                        current: value.into_owned(),
                    }),
                )),
                CrdtValue::U256(value) => unsigned_checker.push(transition.clone_with_data(
                    transition.data.clone_with_states(ValueSnapshots {
                        original: transition.data.value.original.clone(),
                        current: value.into_owned(),
                    }),
                )),
                _ => unreachable!(),
            }
        }
        self.access_checker.import(transitions);
        self.signed_checker.import(signed_checker);
        self.unsigned_checker.import(unsigned_checker);
        let (
            (mut rejected, mut conflicts),
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
        conflicts.extend(signed_pairs);
        conflicts.extend(unsigned_pairs);
        rejected.extend(signed_checker);
        rejected.extend(unsigned_checker);
        rejected.sort_unstable_by_key(|(record, _)| record.id.global);
        self.result.violations = rejected;
        self.result.conflicts = conflicts;

        // Calculate prefix gas only after detection, counting each job once.
        for (id, gas) in self.result.transactions.values_mut() {
            if !self
                .result
                .sequence_gas
                .last()
                .is_some_and(|(previous, _)| {
                    (previous.generation, previous.sequence) == (id.generation, id.sequence)
                })
            {
                self.result.sequence_gas.push((*id, Vec::new()));
            }
            let (_, cumulative) = self.result.sequence_gas.last_mut().unwrap();
            *gas += cumulative.last().copied().unwrap_or_default();
            cumulative.push(*gas);
        }
        for record in self
            .result
            .numeric
            .iter_mut()
            .chain(self.result.violations.iter_mut().map(|(record, _)| record))
        {
            record.accumulated_gas = self.result.transactions
                [&(record.id.generation, record.id.sequence, record.id.job)]
                .1;
        }
        self.result
            .sequence_gas
            .sort_unstable_by(|(left, left_gas), (right, right_gas)| {
                left.generation
                    .cmp(&right.generation)
                    .then_with(|| right_gas.last().cmp(&left_gas.last()))
                    .then_with(|| left.sequence.cmp(&right.sequence))
            });
        std::mem::take(&mut self.result)
    }
}
