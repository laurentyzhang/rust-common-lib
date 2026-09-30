use alloy_primitives::U256 as Number;
use rust_common_lib::{
    crdt::{
        Crdt, I64, U64, U256,
        state::{Delta, DeltaOp, Numeric, Status, Tracked, Value, status::Tag},
    },
    execution::VmCache,
    resolver::{
        ConflictDetector,
        output::ExecutionOutput,
        reason::{AccessConflict, NumericError, Reason},
    },
};

type Record = ExecutionOutput<u64, Tracked<Status, Status>>;

fn combined_records() -> Vec<Record> {
    let mut fallback = VmCache::new(0);
    fallback.insert(&10, Value::from(U64::default())).unwrap();
    fallback
        .insert(&20, Value::from(I64::new(-5, 5).unwrap()))
        .unwrap();

    let mut deleter = VmCache::new_with_fallback(11, &fallback);
    deleter.delete(&10).unwrap();
    let mut reader = VmCache::new_with_fallback(30, &fallback);
    assert!(reader.get(&10).is_some());
    let mut first_delta = VmCache::new_with_fallback(5, &fallback);
    first_delta.add_delta(&20, Delta::I64(3)).unwrap();
    let mut second_delta = VmCache::new_with_fallback(20, &fallback);
    second_delta.add_delta(&20, Delta::I64(4)).unwrap();

    [
        reader.drain().0,
        second_delta.drain().0,
        deleter.drain().0,
        first_delta.drain().0,
    ]
    .into_iter()
    .flatten()
    .collect()
}

#[test]
fn combines_access_and_numeric_rejections_sorted_by_transaction_id() {
    for reverse_import in [false, true] {
        let mut records = combined_records();
        if reverse_import {
            records.reverse();
        }
        let mut detector = ConflictDetector::new();
        // Separate imports must accumulate into both owned stages.
        for record in records {
            detector.import(vec![record]);
        }

        let rejected = detector.detect_conflicts();
        assert_eq!(
            rejected
                .iter()
                .map(|(record, _)| (record.id, record.key))
                .collect::<Vec<_>>(),
            vec![(20, 20), (30, 10)],
        );
        assert!(matches!(
            rejected[0].1,
            Reason::NumericError(NumericError::AboveUpperLimit(_))
        ));
        assert!(matches!(
            rejected[1].1,
            Reason::AccessConflict(AccessConflict::ReadDelete(_))
        ));
    }
}

#[test]
fn preserves_both_reasons_when_both_stages_reject_the_same_record() {
    let mut fallback = VmCache::new(0);
    fallback
        .insert(&7, Value::from(I64::new(-5, 5).unwrap()))
        .unwrap();
    let mut first = VmCache::new_with_fallback(1, &fallback);
    first.add_delta(&7, Delta::I64(3)).unwrap();
    let mut second = VmCache::new_with_fallback(2, &fallback);
    assert!(second.get(&7).is_some());
    second.add_delta(&7, Delta::I64(4)).unwrap();

    let mut detector = ConflictDetector::new();
    detector.import(second.drain().0);
    detector.import(first.drain().0);
    let rejected = detector.detect_conflicts();

    assert_eq!(rejected.len(), 2);
    assert!(
        rejected
            .iter()
            .all(|(record, _)| (record.id, record.key) == (2, 7))
    );
    assert!(rejected.iter().any(|(record, reason)| {
        matches!(reason, Reason::AccessConflict(AccessConflict::ReadWrite(_)))
            && matches!(record.data.current(), Status::Tag(Tag::Stripped))
    }));
    assert!(rejected.iter().any(|(record, reason)| {
        matches!(
            reason,
            Reason::NumericError(NumericError::AboveUpperLimit(_))
        ) && matches!(
            record.data.current(),
            Status::Value(Value::Numeric(Numeric::I64(value)))
                if value.delta().copied() == Some(4)
        )
    }));
}

#[test]
fn consumes_both_stages_and_accepts_a_fresh_batch() {
    let mut detector = ConflictDetector::new();
    assert!(detector.detect_conflicts().is_empty());

    for _ in 0..2 {
        detector.import(combined_records());
        let rejected = detector.detect_conflicts();
        assert_eq!(
            rejected
                .iter()
                .map(|(record, _)| record.id)
                .collect::<Vec<_>>(),
            vec![20, 30],
        );
        assert!(detector.detect_conflicts().is_empty());
    }
}

#[test]
fn numeric_reads_remain_visible_to_access_detection() {
    for (value, delta) in [
        (Value::from(I64::default()), Delta::I64(1)),
        (Value::from(U64::default()), Delta::U64(DeltaOp::Add(1))),
        (
            Value::from(U256::default()),
            Delta::U256(DeltaOp::Add(Number::from(1))),
        ),
    ] {
        let mut fallback = VmCache::new(0);
        fallback.insert(&7, value).unwrap();
        let mut reader = VmCache::new_with_fallback(1, &fallback);
        assert!(reader.get(&7).is_some());
        let mut writer = VmCache::new_with_fallback(2, &fallback);
        writer.add_delta(&7, delta).unwrap();

        let mut detector = ConflictDetector::new();
        detector.import(writer.drain().0);
        detector.import(reader.drain().0);
        let rejected = detector.detect_conflicts();

        assert_eq!(rejected.len(), 1);
        assert_eq!((rejected[0].0.id, rejected[0].0.key), (2, 7));
        assert!(matches!(
            rejected[0].1,
            Reason::AccessConflict(AccessConflict::ReadWrite(_))
        ));
    }
}

#[test]
fn numeric_overflow_and_underflow_reach_the_coordinator_for_every_numeric_type() {
    let mut u64_base = U64::default();
    u64_base.add_delta(&DeltaOp::Add(10)).unwrap();
    u64_base.apply_delta();
    let mut u256_base = U256::default();
    u256_base
        .add_delta(&DeltaOp::Add(Number::from(10)))
        .unwrap();
    u256_base.apply_delta();

    for (value, first_delta, second_delta, underflow) in [
        (
            Value::from(I64::default()),
            Delta::I64(1),
            Delta::I64(i64::MAX),
            false,
        ),
        (
            Value::from(I64::default()),
            Delta::I64(i64::MIN + 1),
            Delta::I64(i64::MIN + 2),
            true,
        ),
        (
            Value::from(U64::default()),
            Delta::U64(DeltaOp::Add(1)),
            Delta::U64(DeltaOp::Add(u64::MAX)),
            false,
        ),
        (
            Value::from(u64_base),
            Delta::U64(DeltaOp::Sub(6)),
            Delta::U64(DeltaOp::Sub(5)),
            true,
        ),
        (
            Value::from(U256::default()),
            Delta::U256(DeltaOp::Add(Number::from(1))),
            Delta::U256(DeltaOp::Add(Number::MAX)),
            false,
        ),
        (
            Value::from(u256_base),
            Delta::U256(DeltaOp::Sub(Number::from(6))),
            Delta::U256(DeltaOp::Sub(Number::from(5))),
            true,
        ),
    ] {
        let mut fallback = VmCache::new(0);
        fallback.insert(&7, value).unwrap();
        let mut first = VmCache::new_with_fallback(1, &fallback);
        first.add_delta(&7, first_delta).unwrap();
        let mut second = VmCache::new_with_fallback(2, &fallback);
        second.add_delta(&7, second_delta).unwrap();

        let mut detector = ConflictDetector::new();
        detector.import(second.drain().0);
        detector.import(first.drain().0);
        let rejected = detector.detect_conflicts();

        assert_eq!(rejected.len(), 1);
        assert_eq!((rejected[0].0.id, rejected[0].0.key), (2, 7));
        assert!(matches!(
            rejected[0].0.data.current(),
            Status::Value(Value::Numeric(_))
        ));
        if underflow {
            assert!(matches!(
                rejected[0].1,
                Reason::NumericError(NumericError::Underflow(_))
            ));
        } else {
            assert!(matches!(
                rejected[0].1,
                Reason::NumericError(NumericError::Overflow(_))
            ));
        }
    }
}
