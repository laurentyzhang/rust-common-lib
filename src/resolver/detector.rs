use super::{
    access_detector::AccessDetector,
    accumulator::Accumulator,
    commit_plan::ConflictResult,
    traits::{ConflictChecker, Rejected},
};
use crate::crdt::state::{
    Tracked,
    status::{Status, Tag},
    value::{Value, Values},
};
use crate::execution::output::ExecutionOutput;
use std::collections::{BTreeSet, HashMap};

/// Coordinates access-conflict detection and numeric accumulation.
pub struct ConflictDetector<K> {
    access_detector: AccessDetector<K>,
    accumulator: Accumulator<K>,
    result: ConflictResult<K>,
}

impl<K> ConflictDetector<K> {
    pub fn new() -> Self {
        Self {
            access_detector: AccessDetector::new(),
            accumulator: Accumulator::new(),
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

        self.access_detector.import(transitions);
        self.accumulator.import(numeric_trans);
    }

    /// Runs both stages in parallel and retains every rejection reason,
    /// including multiple reasons for the same transaction and key.
    pub fn detect_conflicts(&mut self) -> ConflictResult<K>
    where
        K: Eq + std::hash::Hash + Send + Sync,
    {
        let (mut rejected, numeric_rejected) = rayon::join(
            || self.access_detector.detect_conflicts(),
            || self.accumulator.accumulate(),
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
    AccessDetector::<K>::find_rejections(records_by_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crdt::{
        Crdt,
        bytes::Bytes,
        int64::I64,
        state::{Delta, DeltaOp, Numeric, op::Operations, value::Values},
    };
    use crate::execution::VmCache;
    use crate::resolver::reason::{AccessConflict, Reason};
    use std::borrow::Cow;

    fn operations(reads: u32, checks: u32, deltas: u32, deletes: u32) -> Operations {
        Operations {
            reads: reads.into(),
            existence_checks: checks.into(),
            deltas: deltas.into(),
            deletes: deletes.into(),
        }
    }

    fn output(
        id: u64,
        key: u64,
        operations: Operations,
        current: Status,
    ) -> ExecutionOutput<u64, Tracked<Status, Status>> {
        ExecutionOutput {
            id,
            key,
            gas_used: 0,
            data: Tracked {
                id,
                value: Values {
                    original: Status::Tag(Tag::Missing),
                    current,
                },
                operations,
            },
        }
    }

    fn groups(
        records: Vec<ExecutionOutput<u64, Tracked<Status, Status>>>,
    ) -> HashMap<u64, BTreeSet<ExecutionOutput<u64, Tracked<Status, Status>>>> {
        let mut groups = HashMap::new();
        for record in records {
            groups
                .entry(record.key)
                .or_insert_with(BTreeSet::new)
                .insert(record);
        }
        groups
    }

    #[test]
    fn empty_and_single_record_groups_have_no_conflicts_and_are_consumed() {
        let mut empty =
            HashMap::<u64, BTreeSet<ExecutionOutput<u64, Tracked<Status, Status>>>>::new();
        assert!(detect(&mut empty).is_empty());

        let mut records = groups(vec![output(
            1,
            7,
            operations(1, 0, 0, 0),
            Status::Tag(Tag::Default),
        )]);
        assert!(detect(&mut records).is_empty());
        assert!(records[&7].is_empty());
        assert!(detect(&mut records).is_empty());
    }

    #[test]
    fn read_and_delta_on_the_same_key_rejects_the_delta_record() {
        let mut records = groups(vec![
            output(1, 7, operations(1, 0, 0, 0), Status::Tag(Tag::Default)),
            output(2, 7, operations(0, 0, 1, 0), Status::Tag(Tag::Stripped)),
        ]);

        let rejected = detect(&mut records);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0.id, 2);
        assert_eq!(rejected[0].0.key, 7);
        assert!(matches!(
            &rejected[0].1,
            Reason::AccessConflict(AccessConflict::ReadWrite(_))
        ));
    }

    #[test]
    fn delete_and_existence_check_on_the_same_key_conflict() {
        let mut records = groups(vec![
            output(1, 7, operations(0, 0, 0, 1), Status::Tag(Tag::Deleted)),
            output(2, 7, operations(0, 1, 0, 0), Status::Tag(Tag::Default)),
        ]);

        let rejected = detect(&mut records);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0.id, 2);
        assert!(matches!(
            &rejected[0].1,
            Reason::AccessConflict(AccessConflict::ReadWrite(_))
        ));
    }

    #[test]
    fn read_delete_conflict_requires_both_accesses_in_either_direction() {
        for reads in [0, 1, 2] {
            for deletes in [0, 1, 2] {
                let reader = output(1, 7, operations(reads, 0, 0, 0), Status::Tag(Tag::Default));
                let deleter = output(
                    2,
                    7,
                    operations(0, 0, 0, deletes),
                    Status::Tag(Tag::Deleted),
                );
                let expected = reads > 0 && deletes > 0;

                for result in [
                    reader.data.read_conflict_with(&deleter.data),
                    deleter.data.delete_conflict_with(&reader.data),
                ] {
                    assert_eq!(
                        result.is_err(),
                        expected,
                        "reads={reads}, deletes={deletes}"
                    );
                    if let Err(reason) = result {
                        assert!(matches!(
                            reason,
                            Reason::AccessConflict(AccessConflict::ReadDelete(_))
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn vm_read_delete_conflicts_survive_drain_import_and_ordering() {
        for value in [
            Value::from(Bytes::new(vec![1]).unwrap()),
            Value::from(crate::crdt::uint64::U64::default()),
        ] {
            for delete_reads_first in [false, true] {
                for reverse_import in [false, true] {
                    for same_key in [false, true] {
                        let mut fallback = VmCache::new(0);
                        fallback.insert(&7, value.clone()).unwrap();
                        fallback.insert(&8, value.clone()).unwrap();
                        let mut reader = VmCache::new_with_fallback(1, &fallback);
                        let mut deleter = VmCache::new_with_fallback(2, &fallback);
                        let delete_key = if same_key { 7 } else { 8 };

                        assert!((&mut reader).get(&7).is_some());
                        if delete_reads_first {
                            assert!((&mut deleter).get(&delete_key).is_some());
                        }
                        deleter.delete(&delete_key).unwrap();

                        let mut imports = [reader.drain().0, deleter.drain().0];
                        if reverse_import {
                            imports.reverse();
                        }
                        let mut detector = ConflictDetector::new();
                        for records in imports {
                            detector.import(records);
                        }

                        let rejected = detector.detect_conflicts().rejected;
                        assert_eq!(rejected.len(), usize::from(same_key));
                        if same_key {
                            // A pure delete sorts before a read. A read+delete
                            // ties on access counts, so the reader's lower ID wins.
                            assert_eq!(rejected[0].0.id, if delete_reads_first { 2 } else { 1 });
                            assert_eq!(rejected[0].0.key, 7);
                            assert!(matches!(
                                rejected[0].1,
                                Reason::AccessConflict(AccessConflict::ReadDelete(_))
                            ));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn delta_delete_conflict_requires_both_accesses_in_either_direction() {
        for deltas in [0, 1, 2] {
            for deletes in [0, 1, 2] {
                let writer = output(
                    1,
                    7,
                    operations(0, 0, deltas, 0),
                    Status::Tag(Tag::Stripped),
                );
                let deleter = output(
                    2,
                    7,
                    operations(0, 0, 0, deletes),
                    Status::Tag(Tag::Deleted),
                );
                let expected = deltas > 0 && deletes > 0;

                for result in [
                    writer.data.delta_conflict_with(&deleter.data),
                    deleter.data.delete_conflict_with(&writer.data),
                ] {
                    assert_eq!(
                        result.is_err(),
                        expected,
                        "deltas={deltas}, deletes={deletes}"
                    );
                    if let Err(reason) = result {
                        assert!(matches!(
                            reason,
                            Reason::AccessConflict(AccessConflict::DeltaDelete(_))
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn vm_delta_delete_conflicts_survive_drain_import_and_ordering() {
        for (value, delta) in [
            (
                Value::from(Bytes::new(vec![1]).unwrap()),
                Delta::Bytes(vec![2]),
            ),
            (Value::from(I64::default()), Delta::I64(1)),
            (
                Value::from(crate::crdt::uint64::U64::default()),
                Delta::U64(DeltaOp::Add(1)),
            ),
            (
                Value::from(crate::crdt::u256::U256::default()),
                Delta::U256(DeltaOp::Add(alloy_primitives::U256::from(1))),
            ),
            (
                Value::from(crate::crdt::u64_set::U64Set::default()),
                Delta::U64Set(vec![DeltaOp::Add(1)]),
            ),
        ] {
            for delete_writes_first in [false, true] {
                for reverse_import in [false, true] {
                    for same_key in [false, true] {
                        let mut fallback = VmCache::new(0);
                        fallback.insert(&7, value.clone()).unwrap();
                        fallback.insert(&8, value.clone()).unwrap();
                        let mut writer = VmCache::new_with_fallback(1, &fallback);
                        let mut deleter = VmCache::new_with_fallback(2, &fallback);
                        let delete_key = if same_key { 7 } else { 8 };

                        writer.add_delta(&7, delta.clone()).unwrap();
                        if delete_writes_first {
                            deleter.add_delta(&delete_key, delta.clone()).unwrap();
                        }
                        deleter.delete(&delete_key).unwrap();

                        let mut imports = [writer.drain().0, deleter.drain().0];
                        if reverse_import {
                            imports.reverse();
                        }
                        let mut detector = ConflictDetector::new();
                        for records in imports {
                            detector.import(records);
                        }

                        let rejected = detector.detect_conflicts().rejected;
                        assert_eq!(rejected.len(), usize::from(same_key));
                        if same_key {
                            // A pure delete sorts first. With a delta before
                            // deletion, equal delta counts leave the writer first.
                            assert_eq!(rejected[0].0.id, if delete_writes_first { 2 } else { 1 });
                            assert_eq!(rejected[0].0.key, 7);
                            assert!(matches!(
                                rejected[0].1,
                                Reason::AccessConflict(AccessConflict::DeltaDelete(_))
                            ));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn delete_read_and_delta_group_rejects_the_reader_and_writer() {
        let mut records = groups(vec![
            output(1, 7, operations(0, 0, 0, 1), Status::Tag(Tag::Deleted)),
            output(2, 7, operations(1, 0, 0, 0), Status::Tag(Tag::Default)),
            output(3, 7, operations(0, 0, 1, 0), Status::Tag(Tag::Stripped)),
        ]);

        let rejected = detect(&mut records);
        assert_eq!(rejected.len(), 2);
        assert_eq!(rejected[0].0.id, 2);
        assert!(matches!(
            rejected[0].1,
            Reason::AccessConflict(AccessConflict::ReadDelete(_))
        ));
        assert_eq!(rejected[1].0.id, 3);
        assert!(matches!(
            rejected[1].1,
            Reason::AccessConflict(AccessConflict::DeltaDelete(_))
        ));
    }

    #[test]
    fn concurrent_byte_deltas_are_rejected() {
        let mut records = groups(vec![
            output(
                1,
                7,
                operations(0, 0, 1, 0),
                Status::Value(Bytes::new(vec![1]).unwrap().into()),
            ),
            output(
                2,
                7,
                operations(0, 0, 1, 0),
                Status::Value(Bytes::new(vec![2]).unwrap().into()),
            ),
        ]);

        let rejected = detect(&mut records);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0.id, 2);
        assert!(matches!(
            &rejected[0].1,
            Reason::AccessConflict(AccessConflict::ConcurrentDeltaWritesOnNonCommutative(_))
        ));
    }

    #[test]
    fn different_keys_are_isolated_and_results_are_sorted_by_transaction_id() {
        let mut records = groups(vec![
            output(1, 10, operations(1, 0, 0, 0), Status::Tag(Tag::Default)),
            output(4, 10, operations(0, 0, 1, 0), Status::Tag(Tag::Stripped)),
            output(2, 20, operations(0, 0, 0, 1), Status::Tag(Tag::Deleted)),
            output(3, 20, operations(0, 1, 0, 0), Status::Tag(Tag::Default)),
        ]);

        let rejected = detect(&mut records);
        let rejected = rejected
            .into_iter()
            .map(|(record, _)| (record.id, record.key))
            .collect::<Vec<_>>();
        assert_eq!(rejected, vec![(3, 20), (4, 10)]);
    }

    #[test]
    fn no_op_record_does_not_hide_a_later_read_write_conflict() {
        let mut records = groups(vec![
            output(0, 7, operations(0, 0, 0, 0), Status::Tag(Tag::Default)),
            output(1, 7, operations(1, 0, 0, 0), Status::Tag(Tag::Default)),
            output(2, 7, operations(0, 0, 1, 0), Status::Tag(Tag::Stripped)),
        ]);

        let rejected = detect(&mut records);
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].0.id, 2);
        assert!(matches!(
            &rejected[0].1,
            Reason::AccessConflict(AccessConflict::ReadWrite(_))
        ));
    }

    #[test]
    fn import_splits_numeric_data_between_access_and_arithmetic_checks() {
        let mut first = I64::new(-10, 10).unwrap();
        first.add_delta(&3).unwrap();
        let mut second = I64::new(-10, 10).unwrap();
        second.add_delta(&9).unwrap();

        let mut detector = ConflictDetector::new();
        detector.import(vec![
            output(1, 8, operations(0, 0, 0, 0), Status::Tag(Tag::Default)),
            output(
                2,
                7,
                operations(0, 0, 1, 0),
                Status::Value(Value::Numeric(Numeric::I64(Cow::Owned(first)))),
            ),
            output(
                3,
                7,
                operations(1, 0, 1, 0),
                Status::Value(Value::Numeric(Numeric::I64(Cow::Owned(second)))),
            ),
        ]);

        let rejected = detector.detect_conflicts().rejected;
        assert_eq!(rejected.len(), 2);
        assert!(
            rejected
                .iter()
                .all(|(record, _)| record.id == 3 && record.key == 7)
        );
        assert!(rejected.iter().any(|(record, reason)| {
            matches!(record.data.current(), Status::Tag(Tag::Stripped))
                && matches!(reason, Reason::AccessConflict(AccessConflict::ReadWrite(_)))
        }));
        assert!(rejected.iter().any(|(record, reason)| {
            matches!(record.data.current(), Status::Value(Value::Numeric(Numeric::I64(value)))
                if value.delta().copied() == Some(9))
                && matches!(
                    reason,
                    Reason::NumericError(crate::resolver::reason::NumericError::AboveUpperLimit(_))
                )
        }));
    }
}
