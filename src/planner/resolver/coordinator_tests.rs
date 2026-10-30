use super::tests::transition;
use crate::{
    crdt::{
        Crdt, Scalar,
        state::{CrdtDelta, CrdtValue, DeltaOp, Status, Tracked, status::Tag},
    },
    eu::output::ExecutionOutput,
    planner::resolver::{
        ConflictChecker,
        reason::{AccessConflict, NumericError, Reason},
    },
};
use alloy_primitives::{I256, U256 as Number};

type Record = ExecutionOutput<u64, Tracked<Status, Status>>;

fn combined_records() -> Vec<Record> {
    let unsigned = CrdtValue::from(Scalar::<Number>::default());
    let signed = CrdtValue::from(
        Scalar::<I256>::new(I256::unchecked_from(-5), I256::unchecked_from(5)).unwrap(),
    );
    let mut deleter = Tracked::new_owned_existing(unsigned.clone(), 11);
    deleter.remove().unwrap();
    let mut reader = Tracked::new_owned_existing(unsigned, 30);
    assert!(reader.get().is_some());
    let mut first_delta = Tracked::new_owned_existing(signed.clone(), 5);
    first_delta
        .add_delta(CrdtDelta::I256(DeltaOp::Add(Number::from(3))))
        .unwrap();
    let mut second_delta = Tracked::new_owned_existing(signed, 20);
    second_delta
        .add_delta(CrdtDelta::I256(DeltaOp::Add(Number::from(4))))
        .unwrap();
    vec![
        transition(10, reader),
        transition(20, second_delta),
        transition(10, deleter),
        transition(20, first_delta),
    ]
}

#[test]
fn combines_access_and_numeric_rejections_sorted_by_transaction_id() {
    for reverse_import in [false, true] {
        let mut records = combined_records();
        if reverse_import {
            records.reverse();
        }
        let mut detector = ConflictChecker::new();
        // Separate imports must accumulate into both owned stages.
        for record in records {
            detector.import(vec![record]);
        }

        let rejected = detector.check_conflicts().rejected;
        assert_eq!(
            rejected
                .iter()
                .map(|(record, _)| (record.id.global, record.key))
                .collect::<Vec<_>>(),
            vec![(20, 20), (30, 10)],
        );
        assert!(matches!(
            rejected[0].1,
            Reason::NumericError(NumericError::AboveUpperLimit(_))
        ));
        assert!(matches!(
            rejected[1].1,
            Reason::AccessConflict(AccessConflict::RemoveConflict(_))
        ));
    }
}

#[test]
fn conflict_result_builds_commit_and_reject_plans() {
    let mut detector = ConflictChecker::new();
    detector.import(combined_records());

    let result = detector.check_conflicts();
    assert_eq!(
        result.committable.iter().copied().collect::<Vec<_>>(),
        vec![5, 11]
    );

    let commit_plan = result.commit_plan(42, 7);
    assert_eq!(commit_plan.block_id, 42);
    assert_eq!(commit_plan.generation, 7);
    assert_eq!(commit_plan.accepted, vec![5, 11]);

    let reject_plan = result.reject_plan(42, 7);
    assert_eq!(reject_plan.block_id, 42);
    assert_eq!(reject_plan.generation, 7);
    assert_eq!(
        reject_plan
            .rejected
            .iter()
            .map(|(record, _)| (record.id.global, record.key))
            .collect::<Vec<_>>(),
        vec![(20, 20), (30, 10)]
    );
}

#[test]
fn preserves_both_reasons_when_both_stages_reject_the_same_record() {
    let value = CrdtValue::from(
        Scalar::<I256>::new(I256::unchecked_from(-5), I256::unchecked_from(5)).unwrap(),
    );
    let mut first = Tracked::new_owned_existing(value.clone(), 1);
    first
        .add_delta(CrdtDelta::I256(DeltaOp::Add(Number::from(3))))
        .unwrap();
    let mut second = Tracked::new_owned_existing(value, 2);
    assert!(second.get().is_some());
    second
        .add_delta(CrdtDelta::I256(DeltaOp::Add(Number::from(4))))
        .unwrap();
    let mut detector = ConflictChecker::new();
    detector.import(vec![transition(7, second)]);
    detector.import(vec![transition(7, first)]);
    let result = detector.check_conflicts();
    let rejected = &result.rejected;

    assert_eq!(rejected.len(), 2);
    assert!(
        rejected
            .iter()
            .all(|(record, _)| (record.id.global, record.key) == (2, 7))
    );
    assert!(rejected.iter().any(|(record, reason)| {
        matches!(
            reason,
            Reason::AccessConflict(AccessConflict::ReadConflict(_))
        ) && matches!(record.data.current(), Status::Tag(Tag::Stripped))
    }));
    assert!(rejected.iter().any(|(record, reason)| {
        matches!(
            reason,
            Reason::NumericError(NumericError::AboveUpperLimit(_))
        ) && matches!(
            record.data.current(),
            Status::Value(CrdtValue::I256(value))
                if value.delta().copied() == Some(DeltaOp::Add(Number::from(4)))
        )
    }));

    let commit_plan = result.commit_plan(42, 7);
    assert_eq!(commit_plan.accepted, vec![1]);
    let reject_plan = result.reject_plan(42, 7);
    assert_eq!(reject_plan.rejected.len(), 2);
    assert!(
        reject_plan
            .rejected
            .iter()
            .all(|(record, _)| record.id.global == 2)
    );
}

#[test]
fn consumes_both_stages_and_accepts_a_fresh_batch() {
    let mut detector = ConflictChecker::new();
    assert!(detector.check_conflicts().rejected.is_empty());

    for _ in 0..2 {
        detector.import(combined_records());
        let rejected = detector.check_conflicts().rejected;
        assert_eq!(
            rejected
                .iter()
                .map(|(record, _)| record.id.global)
                .collect::<Vec<_>>(),
            vec![20, 30],
        );
        assert!(detector.check_conflicts().rejected.is_empty());
    }
}

#[test]
fn numeric_reads_remain_visible_to_access_detection() {
    for (value, delta) in [
        (
            CrdtValue::from(Scalar::<I256>::default()),
            CrdtDelta::I256(DeltaOp::Add(Number::from(1))),
        ),
        (
            CrdtValue::from(Scalar::<Number>::default()),
            CrdtDelta::U256(DeltaOp::Add(Number::from(1))),
        ),
        (
            CrdtValue::from(Scalar::<Number>::default()),
            CrdtDelta::U256(DeltaOp::Add(Number::from(1))),
        ),
    ] {
        let mut reader = Tracked::new_owned_existing(value.clone(), 1);
        assert!(reader.get().is_some());
        let mut writer = Tracked::new_owned_existing(value, 2);
        writer.add_delta(delta).unwrap();
        let mut detector = ConflictChecker::new();
        detector.import(vec![transition(7, writer)]);
        detector.import(vec![transition(7, reader)]);
        let rejected = detector.check_conflicts().rejected;

        assert_eq!(rejected.len(), 1);
        assert_eq!((rejected[0].0.id.global, rejected[0].0.key), (2, 7));
        assert!(matches!(
            rejected[0].1,
            Reason::AccessConflict(AccessConflict::ReadConflict(_))
        ));
    }
}

#[test]
fn numeric_overflow_and_underflow_reach_the_coordinator_for_every_numeric_type() {
    let mut u64_base = Scalar::<Number>::default();
    u64_base.add_delta(&DeltaOp::Add(Number::from(10))).unwrap();
    u64_base.apply_delta();
    let mut u256_base = Scalar::<Number>::default();
    u256_base
        .add_delta(&DeltaOp::Add(Number::from(10)))
        .unwrap();
    u256_base.apply_delta();

    for (value, first_delta, second_delta, underflow) in [
        (
            CrdtValue::from(Scalar::<I256>::default()),
            CrdtDelta::I256(DeltaOp::Add(Number::from(1))),
            CrdtDelta::I256(DeltaOp::Add(Number::MAX >> 1)),
            false,
        ),
        (
            CrdtValue::from(Scalar::<I256>::default()),
            CrdtDelta::I256(DeltaOp::Sub((Number::from(1) << 255) - Number::from(1))),
            CrdtDelta::I256(DeltaOp::Sub(Number::from(2))),
            true,
        ),
        (
            CrdtValue::from(Scalar::<Number>::default()),
            CrdtDelta::U256(DeltaOp::Add(Number::from(1))),
            CrdtDelta::U256(DeltaOp::Add(Number::MAX)),
            false,
        ),
        (
            CrdtValue::from(u64_base),
            CrdtDelta::U256(DeltaOp::Sub(Number::from(6))),
            CrdtDelta::U256(DeltaOp::Sub(Number::from(5))),
            true,
        ),
        (
            CrdtValue::from(Scalar::<Number>::default()),
            CrdtDelta::U256(DeltaOp::Add(Number::from(1))),
            CrdtDelta::U256(DeltaOp::Add(Number::MAX)),
            false,
        ),
        (
            CrdtValue::from(u256_base),
            CrdtDelta::U256(DeltaOp::Sub(Number::from(6))),
            CrdtDelta::U256(DeltaOp::Sub(Number::from(5))),
            true,
        ),
    ] {
        let mut first = Tracked::new_owned_existing(value.clone(), 1);
        first.add_delta(first_delta).unwrap();
        let mut second = Tracked::new_owned_existing(value, 2);
        second.add_delta(second_delta).unwrap();
        let mut detector = ConflictChecker::new();
        detector.import(vec![transition(7, second)]);
        detector.import(vec![transition(7, first)]);
        let rejected = detector.check_conflicts().rejected;

        assert_eq!(rejected.len(), 1);
        assert_eq!((rejected[0].0.id.global, rejected[0].0.key), (2, 7));
        assert!(matches!(
            rejected[0].0.data.current(),
            Status::Value(CrdtValue::I256(_) | CrdtValue::U256(_))
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
