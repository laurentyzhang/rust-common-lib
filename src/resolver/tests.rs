// Resolver test suites are centralized here.
mod access_checker {
    use crate::crdt::state::op::Operations;
    use crate::crdt::state::{
        Tracked,
        status::{Status, Tag},
        value::Value,
    };
    use crate::execution::output::ExecutionOutput;
    use crate::resolver::access_checker::AccessChecker;
    use crate::resolver::reason::Reason;
    use crate::resolver::traits::Checker;
    use std::collections::BTreeSet;

    fn record(
        original: Tag,
        current: Tag,
        reads: u32,
        existence_checks: u32,
        deltas: u32,
        deletes: u32,
    ) -> Tracked<Status, Status> {
        Tracked {
            id: 0,
            value: crate::crdt::state::value::Values {
                original: Status::Tag(original),
                current: Status::Tag(current),
            },
            operations: Operations {
                reads: reads.into(),
                existence_checks: existence_checks.into(),
                deltas: deltas.into(),
                removes: deletes.into(),
            },
        }
    }

    fn missing() -> Tag {
        Tag::Missing
    }

    fn present() -> Tag {
        Tag::Default
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Operation {
        Creation,
        Existence,
        Read,
        Delta,
        Remove,
    }

    const OPERATIONS: [Operation; 5] = [
        Operation::Creation,
        Operation::Existence,
        Operation::Read,
        Operation::Delta,
        Operation::Remove,
    ];

    fn operation_record(operation: Operation) -> Tracked<Status, Status> {
        match operation {
            Operation::Creation => record(missing(), present(), 0, 0, 1, 0),
            Operation::Existence => record(present(), present(), 0, 1, 0, 0),
            Operation::Read => record(present(), present(), 1, 0, 0, 0),
            Operation::Delta => record(present(), Tag::Stripped, 0, 0, 1, 0),
            Operation::Remove => record(present(), Tag::Deleted, 0, 0, 0, 1),
        }
    }

    fn conflict_from(
        operation: Operation,
        record: &Tracked<Status, Status>,
        other: &Tracked<Status, Status>,
    ) -> Result<(), Reason> {
        match operation {
            Operation::Creation => record.creation_conflict_with(other),
            Operation::Existence => record.existence_conflict_with(other),
            Operation::Read => record.read_conflict_with(other),
            Operation::Delta => record.delta_conflict_with(other),
            Operation::Remove => record.remove_conflict_with(other),
        }
    }

    fn expected_conflict(left: Operation, right: Operation) -> bool {
        use Operation::*;
        matches!(
            (left, right),
            (Creation, Existence | Read | Delta | Remove)
                | (Existence, Creation | Remove)
                | (Read, Creation | Delta | Remove)
                | (Delta, Creation | Read | Remove)
                | (Remove, Creation | Existence | Read | Delta)
        )
    }

    fn count_record(
        reads: u32,
        existence_checks: u32,
        deltas: u32,
        removes: u32,
        missing_original: bool,
        id: u64,
    ) -> ExecutionOutput<u64, Tracked<Status, Status>> {
        let original = if missing_original {
            Tag::Missing
        } else {
            Tag::Default
        };
        let current = if missing_original && (removes > 0 || deltas == 0) {
            Tag::Missing
        } else if removes > 0 {
            Tag::Deleted
        } else if deltas > 0 && !missing_original {
            Tag::Stripped
        } else {
            Tag::Default
        };
        let mut data = record(original, current, reads, existence_checks, deltas, removes);
        data.id = id;

        ExecutionOutput {
            id,
            key: 7,
            gas_used: 0,
            data,
        }
    }

    fn operations_for(record: &Tracked<Status, Status>) -> Vec<Operation> {
        let mut operations = Vec::new();
        let newly_created = record.is_newly_created();
        if newly_created {
            operations.push(Operation::Creation);
        }
        if record.operations.existence_checks.count() > 0 {
            operations.push(Operation::Existence);
        }
        if record.operations.reads.count() > 0 {
            operations.push(Operation::Read);
        }
        if record.operations.deltas.count() > 0 && !newly_created {
            operations.push(Operation::Delta);
        }
        if record.operations.removes.count() > 0 {
            operations.push(Operation::Remove);
        }
        operations
    }

    fn output_for(operation: Operation, id: u64) -> ExecutionOutput<u64, Tracked<Status, Status>> {
        let mut data = operation_record(operation);
        data.id = id;
        ExecutionOutput {
            id,
            key: 7,
            gas_used: 0,
            data,
        }
    }

    #[test]
    fn every_operation_pair_is_checked_in_both_directions_and_by_dispatcher() {
        for left in OPERATIONS {
            for right in OPERATIONS {
                let expected = expected_conflict(left, right);
                let left_record = operation_record(left);
                let right_record = operation_record(right);

                assert_eq!(
                    conflict_from(left, &left_record, &right_record).is_err(),
                    expected,
                    "{left:?} against {right:?}"
                );
                assert_eq!(
                    conflict_from(right, &right_record, &left_record).is_err(),
                    expected,
                    "{right:?} against {left:?}"
                );

                let records = [output_for(left, 1), output_for(right, 2)]
                    .into_iter()
                    .collect::<BTreeSet<_>>();
                let rejected =
                    <AccessChecker<u64> as Checker<u64>>::find_rejections_by_key(records);
                assert_eq!(
                    rejected.len(),
                    usize::from(expected),
                    "dispatcher: {left:?} with {right:?}"
                );
            }
        }
    }

    #[test]
    fn every_access_count_combination_is_checked_in_both_directions() {
        for left_reads in 0..=2 {
            for left_checks in 0..=2 {
                for left_deltas in 0..=2 {
                    for left_removes in 0..=2 {
                        for left_missing in [false, true] {
                            for right_reads in 0..=2 {
                                for right_checks in 0..=2 {
                                    for right_deltas in 0..=2 {
                                        for right_removes in 0..=2 {
                                            for right_missing in [false, true] {
                                                for (left_id, right_id) in [(1, 2), (2, 1)] {
                                                    let left = count_record(
                                                        left_reads,
                                                        left_checks,
                                                        left_deltas,
                                                        left_removes,
                                                        left_missing,
                                                        left_id,
                                                    );
                                                    let right = count_record(
                                                        right_reads,
                                                        right_checks,
                                                        right_deltas,
                                                        right_removes,
                                                        right_missing,
                                                        right_id,
                                                    );
                                                    let left_operations =
                                                        operations_for(&left.data);
                                                    let right_operations =
                                                        operations_for(&right.data);
                                                    let expected = left_operations.iter().any(
                                                        |left_operation| {
                                                            right_operations.iter().any(
                                                                |right_operation| {
                                                                    expected_conflict(
                                                                        *left_operation,
                                                                        *right_operation,
                                                                    )
                                                                },
                                                            )
                                                        },
                                                    );

                                                    for left_operation in &left_operations {
                                                        for right_operation in &right_operations {
                                                            let expected_from_left =
                                                                right_operations.iter().any(
                                                                    |right_operation| {
                                                                        expected_conflict(
                                                                            *left_operation,
                                                                            *right_operation,
                                                                        )
                                                                    },
                                                                );
                                                            let expected_from_right =
                                                                left_operations.iter().any(
                                                                    |left_operation| {
                                                                        expected_conflict(
                                                                            *right_operation,
                                                                            *left_operation,
                                                                        )
                                                                    },
                                                                );
                                                            assert_eq!(
                                                                conflict_from(
                                                                    *left_operation,
                                                                    &left.data,
                                                                    &right.data
                                                                )
                                                                .is_err(),
                                                                expected_from_left,
                                                                "direct: left=({left_reads},{left_checks},{left_deltas},{left_removes},{left_missing}), right=({right_reads},{right_checks},{right_deltas},{right_removes},{right_missing}), ops=({left_operation:?},{right_operation:?})"
                                                            );
                                                            assert_eq!(
                                                                conflict_from(
                                                                    *right_operation,
                                                                    &right.data,
                                                                    &left.data
                                                                )
                                                                .is_err(),
                                                                expected_from_right,
                                                                "reverse direct: left=({left_reads},{left_checks},{left_deltas},{left_removes},{left_missing}), right=({right_reads},{right_checks},{right_deltas},{right_removes},{right_missing}), ops=({right_operation:?},{left_operation:?})"
                                                            );
                                                        }
                                                    }

                                                    let records = [left, right]
                                                        .into_iter()
                                                        .collect::<BTreeSet<_>>();
                                                    let rejected =
                                                        <AccessChecker<u64> as Checker<u64>>::find_rejections_by_key(records);
                                                    assert_eq!(
                                                        rejected.len(),
                                                        usize::from(expected),
                                                        "dispatcher: left=({left_reads},{left_checks},{left_deltas},{left_removes},{left_missing}), right=({right_reads},{right_checks},{right_deltas},{right_removes},{right_missing}), ids=({left_id},{right_id})"
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn creations_conflict_only_when_the_created_values_differ() {
        use crate::crdt::bytes::Bytes;
        use std::borrow::Cow;

        let created_value = |id, bytes| {
            let mut data = record(missing(), present(), 0, 0, 1, 0);
            data.id = id;
            data.value.current =
                Status::Value(Value::Bytes(Cow::Owned(Bytes::new(vec![bytes]).unwrap())));
            ExecutionOutput {
                id,
                key: 7,
                gas_used: 0,
                data,
            }
        };

        let same_value = [created_value(1, 1), created_value(2, 1)]
            .into_iter()
            .collect::<BTreeSet<_>>();
        assert!(
            <AccessChecker<u64> as Checker<u64>>::find_rejections_by_key(same_value).is_empty()
        );

        for (left_id, right_id) in [(1, 2), (2, 1)] {
            let different_values = [created_value(left_id, 1), created_value(right_id, 2)]
                .into_iter()
                .collect::<BTreeSet<_>>();
            let rejected =
                <AccessChecker<u64> as Checker<u64>>::find_rejections_by_key(different_values);
            assert_eq!(rejected.len(), 1, "creation IDs=({left_id}, {right_id})");
            assert_eq!(rejected[0].0.id, left_id.max(right_id));
        }
    }

    #[test]
    fn concurrent_byte_deltas_conflict_in_both_directions() {
        use crate::crdt::bytes::Bytes;
        use std::borrow::Cow;

        let mut left = record(present(), Tag::Stripped, 0, 0, 1, 0);
        let mut right = record(present(), Tag::Stripped, 0, 0, 1, 0);
        left.value.current = Status::Value(Value::Bytes(Cow::Owned(Bytes::new(vec![1]).unwrap())));
        right.value.current = Status::Value(Value::Bytes(Cow::Owned(Bytes::new(vec![2]).unwrap())));

        assert!(left.delta_conflict_with(&right).is_err());
        assert!(right.delta_conflict_with(&left).is_err());
    }

    #[test]
    fn creation_conflicts_with_existence_check_in_both_directions() {
        let creation = record(missing(), present(), 0, 0, 1, 0);
        let existence_check = record(missing(), missing(), 0, 1, 0, 0);

        assert!(creation.creation_conflict_with(&existence_check).is_err());
        assert!(existence_check.existence_conflict_with(&creation).is_err());
    }

    #[test]
    fn existence_check_does_not_conflict_with_delta_to_existing_value() {
        let existence_check = record(present(), present(), 0, 1, 0, 0);
        let delta = record(present(), present(), 0, 0, 1, 0);

        assert!(existence_check.existence_conflict_with(&delta).is_ok());
        assert!(delta.delta_conflict_with(&existence_check).is_ok());
    }

    #[test]
    fn existence_check_does_not_conflict_with_value_read() {
        let existence_check = record(present(), present(), 0, 1, 0, 0);
        let read = record(present(), present(), 1, 0, 0, 0);

        assert!(existence_check.existence_conflict_with(&read).is_ok());
        assert!(read.read_conflict_with(&existence_check).is_ok());
    }

    #[test]
    fn existence_check_conflicts_with_remove_in_both_directions() {
        let existence_check = record(present(), present(), 0, 1, 0, 0);
        let removal = record(present(), Tag::Deleted, 0, 0, 0, 1);

        assert!(existence_check.existence_conflict_with(&removal).is_err());
        assert!(removal.remove_conflict_with(&existence_check).is_err());
    }

    #[test]
    fn read_remove_conflicts_for_every_operation_count_in_both_directions() {
        for reads in [0, 1, 2] {
            for removes in [0, 1, 2] {
                let reader = record(present(), present(), reads, 0, 0, 0);
                let remover = record(present(), Tag::Deleted, 0, 0, 0, removes);
                let expected = reads > 0 && removes > 0;

                for (result, expected_reason) in [
                    (reader.read_conflict_with(&remover), true),
                    (remover.remove_conflict_with(&reader), false),
                ] {
                    assert_eq!(
                        result.is_err(),
                        expected,
                        "reads={reads}, removes={removes}"
                    );
                    if let Err(reason) = result {
                        let Reason::AccessConflict(reason) = reason else {
                            panic!("unexpected conflict reason: {reason:?}");
                        };
                        assert!(if expected_reason {
                            matches!(
                                reason,
                                crate::resolver::reason::AccessConflict::RemoveConflict(_)
                            )
                        } else {
                            matches!(
                                reason,
                                crate::resolver::reason::AccessConflict::ReadConflict(_)
                            )
                        });
                    }
                }
            }
        }
    }

    #[test]
    fn delta_remove_conflicts_for_every_operation_count_in_both_directions() {
        for deltas in [0, 1, 2] {
            for removes in [0, 1, 2] {
                let writer = record(present(), Tag::Stripped, 0, 0, deltas, 0);
                let remover = record(present(), Tag::Deleted, 0, 0, 0, removes);
                let expected = deltas > 0 && removes > 0;

                for (result, expected_reason) in [
                    (writer.delta_conflict_with(&remover), true),
                    (remover.remove_conflict_with(&writer), false),
                ] {
                    assert_eq!(
                        result.is_err(),
                        expected,
                        "deltas={deltas}, removes={removes}"
                    );
                    if let Err(reason) = result {
                        let Reason::AccessConflict(reason) = reason else {
                            panic!("unexpected conflict reason: {reason:?}");
                        };
                        assert!(if expected_reason {
                            matches!(
                                reason,
                                crate::resolver::reason::AccessConflict::RemoveConflict(_)
                            )
                        } else {
                            matches!(
                                reason,
                                crate::resolver::reason::AccessConflict::DeltaConflict(_)
                            )
                        });
                    }
                }
            }
        }
    }
}

mod conflict_checker {
    use crate::crdt::{
        Crdt,
        bytes::Bytes,
        int64::I64,
        state::{
            Delta, DeltaOp, Numeric, Tracked,
            op::Operations,
            status::{Status, Tag},
            value::{Value, Values},
        },
    };
    use crate::execution::VmCache;
    use crate::execution::output::ExecutionOutput;
    use crate::resolver::access_checker::AccessChecker;
    use crate::resolver::conflict_checker::*;
    use crate::resolver::reason::{AccessConflict, Reason};
    use crate::resolver::traits::Checker;
    use std::borrow::Cow;
    use std::collections::{BTreeSet, HashMap};

    fn operations(reads: u32, checks: u32, deltas: u32, deletes: u32) -> Operations {
        Operations {
            reads: reads.into(),
            existence_checks: checks.into(),
            deltas: deltas.into(),
            removes: deletes.into(),
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

    fn detect(
        records: &mut HashMap<u64, BTreeSet<ExecutionOutput<u64, Tracked<Status, Status>>>>,
    ) -> Vec<crate::resolver::traits::Rejected<u64>> {
        <AccessChecker<u64> as Checker<u64>>::find_rejections(records)
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
            Reason::AccessConflict(AccessConflict::ReadConflict(_))
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
    }

    #[test]
    fn vm_read_remove_conflicts_survive_drain_import_and_ordering() {
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
                        let mut detector = ConflictChecker::new();
                        for records in imports {
                            detector.import(records);
                        }

                        let rejected = detector.check_conflicts().rejected;
                        assert_eq!(rejected.len(), usize::from(same_key));
                        if same_key {
                            // A pure delete sorts before a read. A read+delete
                            // ties on access counts, so the reader's lower ID wins.
                            assert_eq!(rejected[0].0.id, if delete_reads_first { 2 } else { 1 });
                            assert_eq!(rejected[0].0.key, 7);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn vm_delta_remove_conflicts_survive_drain_import_and_ordering() {
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
                        let mut detector = ConflictChecker::new();
                        for records in imports {
                            detector.import(records);
                        }

                        let rejected = detector.check_conflicts().rejected;
                        assert_eq!(rejected.len(), usize::from(same_key));
                        if same_key {
                            // A pure delete sorts first. With a delta before
                            // deletion, equal delta counts leave the writer first.
                            assert_eq!(rejected[0].0.id, if delete_writes_first { 2 } else { 1 });
                            assert_eq!(rejected[0].0.key, 7);
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
            Reason::AccessConflict(AccessConflict::ReadConflict(_))
        ));
        assert_eq!(rejected[1].0.id, 3);
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
            Reason::AccessConflict(AccessConflict::ReadConflict(_))
        ));
    }

    #[test]
    fn import_splits_numeric_data_between_access_and_arithmetic_checks() {
        let mut first = I64::new(-10, 10).unwrap();
        first.add_delta(&3).unwrap();
        let mut second = I64::new(-10, 10).unwrap();
        second.add_delta(&9).unwrap();

        let mut detector = ConflictChecker::new();
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

        let rejected = detector.check_conflicts().rejected;
        assert_eq!(rejected.len(), 2);
        assert!(
            rejected
                .iter()
                .all(|(record, _)| record.id == 3 && record.key == 7)
        );
        assert!(rejected.iter().any(|(record, reason)| {
            matches!(record.data.current(), Status::Tag(Tag::Stripped))
                && matches!(
                    reason,
                    Reason::AccessConflict(AccessConflict::ReadConflict(_))
                )
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

mod sum_checker {
    use crate::crdt::{
        Crdt,
        int64::I64,
        state::{Delta, DeltaOp, Numeric, Status, Tracked, Value, status::Tag, value::Values},
        u256::U256,
        uint64::U64,
    };
    use crate::execution::output::ExecutionOutput;
    use crate::resolver::reason::Reason;
    use crate::resolver::sum_checker::*;
    use std::borrow::Cow;
    use std::collections::{BTreeSet, HashMap};

    fn u256_value(delta: alloy_primitives::U256) -> Numeric<'static> {
        let mut value = U256::default();
        value.add_delta(&DeltaOp::Add(delta)).unwrap();
        Numeric::U256(Cow::Owned(value))
    }

    fn u256_operation(operation: DeltaOp<alloy_primitives::U256>) -> Numeric<'static> {
        let mut value = U256::default();
        value
            .add_delta(&DeltaOp::Add(alloy_primitives::U256::from(10)))
            .unwrap();
        value.apply_delta();
        value.add_delta(&operation).unwrap();
        Numeric::U256(Cow::Owned(value))
    }

    fn u64_value(delta: u64) -> Numeric<'static> {
        let mut value = U64::default();
        value.add_delta(&DeltaOp::Add(delta)).unwrap();
        Numeric::U64(Cow::Owned(value))
    }

    fn u64_operation(operation: DeltaOp<u64>) -> Numeric<'static> {
        let mut value = U64::default();
        value.add_delta(&DeltaOp::Add(10)).unwrap();
        value.apply_delta();
        value.add_delta(&operation).unwrap();
        Numeric::U64(Cow::Owned(value))
    }

    fn i64_value(delta: i64, lower: i64, upper: i64) -> Numeric<'static> {
        let mut value = I64::new(lower, upper).unwrap();
        value.add_delta(&delta).unwrap();
        Numeric::I64(Cow::Owned(value))
    }

    fn execution<K>(
        tx_id: u64,
        records: Vec<(K, Numeric<'static>)>,
    ) -> HashMap<K, BTreeSet<ExecutionOutput<K, Tracked<Status, Numeric<'static>>>>>
    where
        K: Clone + Eq + std::hash::Hash,
    {
        let mut outputs = HashMap::new();

        for (key, value) in records {
            outputs
                .entry(key.clone())
                .or_insert_with(BTreeSet::new)
                .insert(ExecutionOutput {
                    id: tx_id,
                    key,
                    gas_used: 0,
                    data: Tracked::<Value<'static>, Value<'static>>::new_owned_empty(tx_id)
                        .clone_with_states(Values {
                            original: Status::Tag(Tag::Missing),
                            current: value,
                        }),
                });
        }

        outputs
    }

    fn assert_mixed_sign_rejections(
        base: Numeric<'static>,
        deltas: Vec<Delta>,
        underflow_id: u64,
        overflow_ids: &[u64],
    ) {
        // Each transaction starts from the same committed value and is valid
        // on its own. Only combining transactions may exceed the type's range.
        let values = deltas
            .iter()
            .map(|delta| {
                let mut value = base.clone();
                value
                    .add_delta(delta)
                    .expect("individual delta must be valid");
                value
            })
            .collect::<Vec<_>>();

        // Check every sorted prefix, so accepted deltas after underflow and
        // the precise point of overflow are verified, in either import order.
        for prefix_len in 1..=values.len() {
            for reverse_import in [false, true] {
                let mut sum_checker = SumChecker::new();
                let mut indices = (0..prefix_len).collect::<Vec<_>>();
                if reverse_import {
                    indices.reverse();
                }
                for index in indices {
                    sum_checker.import(execution(
                        index as u64 + 1,
                        vec![(7, values[index].clone())],
                    ));
                }

                assert_eq!(
                    sum_checker.entries[&7]
                        .iter()
                        .map(|record| record.id)
                        .collect::<Vec<_>>(),
                    (1..=prefix_len as u64).collect::<Vec<_>>(),
                );
                let rejected = sum_checker.accumulate();
                let expected_ids = std::iter::once(underflow_id)
                    .chain(overflow_ids.iter().copied())
                    .filter(|id| *id <= prefix_len as u64)
                    .collect::<Vec<_>>();
                assert_eq!(
                    rejected
                        .iter()
                        .map(|(record, _)| record.id)
                        .collect::<Vec<_>>(),
                    expected_ids,
                    "prefix_len={prefix_len}, reverse_import={reverse_import}",
                );

                for (record, reason) in rejected {
                    assert_eq!(record.key, 7);
                    assert!(matches!(
                        record.data.current(),
                        Status::Value(Value::Numeric(value)) if value == &values[record.id as usize - 1]
                    ));
                    if record.id == underflow_id {
                        assert!(matches!(
                            reason,
                            Reason::NumericError(crate::resolver::reason::NumericError::Underflow(
                                _
                            ))
                        ));
                    } else {
                        assert!(matches!(
                            reason,
                            Reason::NumericError(crate::resolver::reason::NumericError::Overflow(
                                _
                            ))
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn accumulate_i64_mixed_signs_reports_underflow_and_overflow() {
        for (initial, deltas, overflow_ids) in [
            // Pending delta reaches MIN, cancels to zero, then reaches MAX.
            (
                0,
                vec![
                    i64::MIN + 1,
                    i64::MIN + 2,
                    -1,
                    0,
                    1,
                    i64::MAX,
                    i64::MAX,
                    i64::MAX,
                    i64::MAX,
                ],
                [8, 9],
            ),
            // Underflow occurs in stored value + pending delta. Later deltas
            // reach MIN, cancel, reach pending MAX, and overflow pending delta.
            (
                -10,
                vec![
                    i64::MIN + 11,
                    -2,
                    -1,
                    0,
                    1,
                    i64::MAX - 10,
                    i64::MAX,
                    i64::MAX,
                    i64::MAX,
                ],
                [8, 9],
            ),
            // Underflow occurs in pending delta; overflow occurs when adding
            // a representable pending delta (MAX - 2) to the stored value 10.
            (
                10,
                vec![
                    i64::MIN + 1,
                    i64::MIN + 2,
                    -1,
                    0,
                    1,
                    i64::MAX / 2,
                    i64::MAX / 2,
                    i64::MAX / 2,
                    i64::MAX / 2,
                    i64::MAX / 2,
                ],
                [9, 10],
            ),
        ] {
            let mut base = I64::default();
            base.add_delta(&initial).unwrap();
            base.apply_delta();
            assert_mixed_sign_rejections(
                Numeric::I64(Cow::Owned(base)),
                deltas.into_iter().map(Delta::I64).collect(),
                2,
                &overflow_ids,
            );
        }
    }

    #[test]
    fn accumulate_u64_mixed_signs_reports_underflow_and_overflow() {
        let mut base = U64::default();
        base.add_delta(&DeltaOp::Add(10)).unwrap();
        base.apply_delta();

        // 10 - 6 = 4; -5 underflows; -4 reaches zero; +10 cancels;
        // +(MAX - 10) reaches MAX. Both following additions overflow.
        let deltas = [
            DeltaOp::Sub(6),
            DeltaOp::Sub(5),
            DeltaOp::Sub(4),
            DeltaOp::Add(0),
            DeltaOp::Add(10),
            DeltaOp::Add(u64::MAX - 10),
            DeltaOp::Add(u64::MAX - 10),
            DeltaOp::Add(u64::MAX - 10),
        ];
        assert_mixed_sign_rejections(
            Numeric::U64(Cow::Owned(base)),
            deltas.into_iter().map(Delta::U64).collect(),
            2,
            &[7, 8],
        );
    }

    #[test]
    fn accumulate_u256_mixed_signs_reports_underflow_and_overflow() {
        use alloy_primitives::U256 as Number;

        let mut base = U256::default();
        base.add_delta(&DeltaOp::Add(Number::from(10))).unwrap();
        base.apply_delta();

        // Exercise the full 256-bit range with the same recovery and
        // cancellation sequence as U64, including repeated overflow.
        let large = Number::MAX - Number::from(10);
        let deltas = [
            DeltaOp::Sub(Number::from(6)),
            DeltaOp::Sub(Number::from(5)),
            DeltaOp::Sub(Number::from(4)),
            DeltaOp::Add(Number::ZERO),
            DeltaOp::Add(Number::from(10)),
            DeltaOp::Add(large),
            DeltaOp::Add(large),
            DeltaOp::Add(large),
        ];
        assert_mixed_sign_rejections(
            Numeric::U256(Cow::Owned(base)),
            deltas.into_iter().map(Delta::U256).collect(),
            2,
            &[7, 8],
        );
    }

    #[test]
    fn sort_entries_orders_numeric_deltas_then_transaction_id() {
        let mut sum_checker = SumChecker::new();
        let saturated = alloy_primitives::U256::from(u64::MAX);

        sum_checker.import(execution(
            30,
            vec![(1, u256_value(alloy_primitives::U256::from(5)))],
        ));
        sum_checker.import(execution(
            20,
            vec![(1, u256_value(alloy_primitives::U256::from(5)))],
        ));
        sum_checker.import(execution(
            1,
            vec![(1, u256_value(saturated + alloy_primitives::U256::from(2)))],
        ));
        sum_checker.import(execution(
            90,
            vec![(1, u256_value(saturated + alloy_primitives::U256::from(1)))],
        ));
        let tx_ids = sum_checker.entries[&1]
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>();

        assert_eq!(tx_ids, vec![20, 30, 90, 1]);
    }

    #[test]
    fn sort_entries_accounts_for_delta_sign() {
        let mut sum_checker = SumChecker::new();

        sum_checker.import(execution(
            30,
            vec![(
                1,
                u256_operation(DeltaOp::Sub(alloy_primitives::U256::from(2))),
            )],
        ));
        sum_checker.import(execution(
            40,
            vec![(
                1,
                u256_operation(DeltaOp::Sub(alloy_primitives::U256::from(5))),
            )],
        ));
        sum_checker.import(execution(
            20,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(2))),
            )],
        ));
        sum_checker.import(execution(
            10,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(2))),
            )],
        ));
        sum_checker.import(execution(
            50,
            vec![(
                1,
                u256_operation(DeltaOp::Add(alloy_primitives::U256::from(5))),
            )],
        ));

        let tx_ids = sum_checker.entries[&1]
            .iter()
            .map(|record| record.id)
            .collect::<Vec<_>>();

        assert_eq!(tx_ids, vec![40, 30, 10, 20, 50]);
    }

    #[test]
    fn accumulate_returns_rejected_transactions_and_continues() {
        let mut sum_checker = SumChecker::new();

        sum_checker.import(execution(1, vec![(1, i64_value(3, -5, 5))]));
        sum_checker.import(execution(2, vec![(1, i64_value(4, -5, 5))]));
        sum_checker.import(execution(3, vec![(1, i64_value(5, -5, 5))]));
        sum_checker.import(execution(4, vec![(2, i64_value(2, -5, 5))]));
        sum_checker.import(execution(5, vec![(2, i64_value(4, -5, 5))]));
        sum_checker.import(execution(6, vec![(3, i64_value(5, -5, 5))]));

        let rejected = sum_checker.accumulate();
        let rejected = rejected
            .into_iter()
            .map(|(record, _)| record.id)
            .collect::<Vec<_>>();

        assert_eq!(rejected, vec![2, 3, 5]);
    }

    #[test]
    fn empty_and_single_record_groups_have_no_rejections() {
        let mut sum_checker = SumChecker::<u64>::new();
        assert!(sum_checker.accumulate().is_empty());

        sum_checker.import(execution(1, vec![(7, i64_value(3, -5, 5))]));
        assert!(sum_checker.accumulate().is_empty());
        assert!(sum_checker.entries[&7].is_empty());
        assert!(sum_checker.accumulate().is_empty());
    }

    #[test]
    fn rejection_retains_key_numeric_data_and_reason() {
        let mut sum_checker = SumChecker::new();
        sum_checker.import(execution(1, vec![(7, i64_value(3, -5, 5))]));
        sum_checker.import(execution(2, vec![(7, i64_value(4, -5, 5))]));

        let rejected = sum_checker.accumulate();
        assert_eq!(rejected.len(), 1);

        let (record, reason) = &rejected[0];
        assert_eq!(record.id, 2);
        assert_eq!(record.key, 7);
        assert!(matches!(
            record.data.current(),
            Status::Value(Value::Numeric(Numeric::I64(value)))
                if value.delta().copied() == Some(4)
        ));
        assert!(matches!(
            reason,
            Reason::NumericError(crate::resolver::reason::NumericError::AboveUpperLimit(_))
        ));
    }

    #[test]
    fn same_transaction_rejected_for_multiple_keys_keeps_every_record() {
        let mut sum_checker = SumChecker::new();
        sum_checker.import(execution(1, vec![(10, i64_value(3, -5, 5))]));
        sum_checker.import(execution(2, vec![(20, i64_value(3, -5, 5))]));
        sum_checker.import(execution(
            7,
            vec![(10, i64_value(4, -5, 5)), (20, i64_value(4, -5, 5))],
        ));

        let rejected = sum_checker.accumulate();
        assert_eq!(rejected.len(), 2);
        assert!(rejected.iter().all(|(record, _)| record.id == 7));

        let mut keys = rejected
            .into_iter()
            .map(|(record, _)| record.key)
            .collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, vec![10, 20]);
    }

    #[test]
    fn accumulate_reports_arithmetic_overflow() {
        let mut sum_checker = SumChecker::new();
        sum_checker.import(execution(1, vec![(7, u64_value(1))]));
        sum_checker.import(execution(2, vec![(7, u64_value(u64::MAX))]));

        let rejected = sum_checker.accumulate();
        assert_eq!(rejected.len(), 1);

        let (record, reason) = &rejected[0];
        assert_eq!(record.id, 2);
        assert_eq!(record.key, 7);
        assert!(matches!(
            record.data.current(),
            Status::Value(Value::Numeric(Numeric::U64(value)))
                if value.delta() == Some(&DeltaOp::Add(u64::MAX))
        ));
        assert!(matches!(
            reason,
            Reason::NumericError(crate::resolver::reason::NumericError::Overflow(_))
        ));
    }

    #[test]
    fn accumulate_reports_arithmetic_underflow() {
        let mut sum_checker = SumChecker::new();
        sum_checker.import(execution(1, vec![(7, u64_operation(DeltaOp::Sub(6)))]));
        sum_checker.import(execution(2, vec![(7, u64_operation(DeltaOp::Sub(5)))]));

        let rejected = sum_checker.accumulate();
        assert_eq!(rejected.len(), 1);

        let (record, reason) = &rejected[0];
        assert_eq!(record.id, 2);
        assert_eq!(record.key, 7);
        assert!(matches!(
            record.data.current(),
            Status::Value(Value::Numeric(Numeric::U64(value)))
                if value.delta() == Some(&DeltaOp::Sub(5))
        ));
        assert!(matches!(
            reason,
            Reason::NumericError(crate::resolver::reason::NumericError::Underflow(_))
        ));
    }
}
