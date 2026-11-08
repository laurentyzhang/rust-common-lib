use crate::{
    crdt::{
        Crdt, Scalar,
        state::{
            CrdtDelta, CrdtValue, DeltaOp, Status, Tracked, status::Tag, value::ValueSnapshots,
        },
    },
    execution::output::ExecutionOutput,
    planner::{
        resolver::{
            AccessConflict, Checker, CommitVerdict, ConflictDetector, NumericError, Reason,
            accepted_ids, rejected_ids,
        },
        scheduler::ID,
    },
};
use alloy_primitives::{I256, U256};
use std::collections::BTreeSet;

fn pairs<K>(verdicts: &[CommitVerdict<K>]) -> BTreeSet<(ID, ID)> {
    verdicts
        .iter()
        .flat_map(|verdict| {
            verdict
                .peers
                .iter()
                .map(move |peer| (verdict.id.min(*peer), verdict.id.max(*peer)))
        })
        .collect()
}

pub(super) fn transition(
    key: u64,
    data: Tracked<CrdtValue<'static>, CrdtValue<'static>>,
) -> ExecutionOutput<u64, Tracked<Status, Status>> {
    let snapshots = crate::crdt::state::status::reduce_to_statuses(&data.value);
    ExecutionOutput {
        id: ID {
            sequence: data.id,
            ..ID::new(0, data.id)
        },
        key,
        gas_used: 0,
        accumulated_gas: 0,
        data: data.clone_with_states(snapshots),
    }
}

fn record(
    global: u64,
    sequence: u64,
    job: u64,
    key: u64,
    gas_used: u64,
) -> ExecutionOutput<u64, Tracked<Status, Status>> {
    let mut data = Tracked::new_owned_existing(CrdtValue::from(Scalar::<U256>::default()), global);
    assert!(data.get().is_some());
    data.add_delta(CrdtDelta::U256(DeltaOp::Add(U256::from(1))))
        .unwrap();
    ExecutionOutput {
        id: ID {
            block: 0,
            global,
            sequence,
            job,
            generation: 0,
        },
        key,
        gas_used,
        accumulated_gas: 0,
        data: data.clone_with_states(ValueSnapshots {
            original: Status::Tag(Tag::Stripped),
            current: Status::Tag(Tag::Stripped),
        }),
    }
}

#[test]
fn reconstructs_sequences_and_counts_each_transaction_once() {
    for reverse in [false, true] {
        let mut records = vec![
            record(1, 0, 0, 1, 20),
            record(2, 0, 1, 100, 20),
            record(2, 0, 1, 200, 20),
            record(2, 0, 1, 201, 20),
            record(3, 0, 2, 3, 20),
            record(4, 1, 0, 100, 30),
            record(5, 1, 1, 5, 30),
            record(6, 1, 2, 6, 30),
        ];
        if reverse {
            records.reverse();
        }
        let mut checker = ConflictDetector::new();
        for record in records {
            checker.import(vec![record]);
        }
        let report = checker.check();
        assert_eq!(
            accepted_ids(&report)
                .into_iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![1, 4, 5, 6]
        );
        assert_eq!(report.len(), 6);
        assert_eq!(
            pairs(&report)
                .iter()
                .map(|(left, right)| (left.global, right.global))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(2, 4)])
        );
        assert!(checker.check().is_empty());
    }
}

#[test]
fn reports_every_conflicting_pair() {
    let mut checker = ConflictDetector::new();
    checker.import(vec![
        record(1, 0, 0, 7, 1),
        record(2, 1, 0, 7, 10),
        record(3, 2, 0, 7, 2),
    ]);
    let report = checker.check();
    assert_eq!(
        pairs(&report)
            .iter()
            .map(|(left, right)| (left.global, right.global))
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([(1, 2), (1, 3), (2, 3)])
    );
    assert_eq!(
        accepted_ids(&report)
            .into_iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(
        report[1]
            .peers
            .iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![1, 3]
    );
    assert!(!report[1].reasons.is_empty());
}

#[test]
fn checks_numeric_bounds_across_multiple_imports() {
    let mut records = Vec::new();
    for (index, (delta, gas)) in [(1, 1), (6, 1), (7, 20), (8, 10)].into_iter().enumerate() {
        let id = index as u64 + 1;
        let scalar = Scalar::<U256>::new(U256::ZERO, U256::from(10)).unwrap();
        let mut data = Tracked::new_owned_existing(CrdtValue::from(scalar), id);
        data.add_delta(CrdtDelta::U256(DeltaOp::Add(U256::from(delta))))
            .unwrap();
        records.push(ExecutionOutput {
            id: ID {
                sequence: index as u64,
                ..ID::new(0, id)
            },
            key: 7_u64,
            gas_used: gas,
            accumulated_gas: 0,
            data: data.clone_with_states(ValueSnapshots {
                original: Status::Tag(Tag::Stripped),
                current: Status::Value(data.current().clone().into_owned()),
            }),
        });
    }
    let mut checker = ConflictDetector::new();
    for record in records {
        checker.import(vec![record]);
    }
    let result = checker.check();
    assert_eq!(
        accepted_ids(&result)
            .into_iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(
        rejected_ids(&result)
            .into_iter()
            .map(|id| id.global)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([3, 4])
    );
}

#[test]
fn clears_previous_generation_metadata() {
    let mut checker = ConflictDetector::new();
    for generation in 0..2 {
        let mut records = vec![record(1, 0, 0, 1, 10), record(2, 1, 0, 2, 20)];
        for record in &mut records {
            record.id.generation = generation;
        }
        checker.import(records);
        let result = checker.check();
        assert_eq!(
            accepted_ids(&result)
                .into_iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(
            result
                .iter()
                .all(|verdict| verdict.id.generation == generation)
        );
    }
    assert!(checker.check().is_empty());
}

#[test]
fn orders_jobs_by_coordinates_and_preserves_all_key_records() {
    let mut checker = ConflictDetector::new();
    checker.import(vec![record(10, 0, 2, 10, 30), record(30, 0, 0, 30, 10)]);
    checker.import(vec![
        record(20, 0, 1, 20, 20),
        record(20, 0, 1, 99, 20),
        record(40, 1, 0, 99, 15),
    ]);
    let report = checker.check();
    assert_eq!(report.len(), 4);
    assert!(
        pairs(&report)
            .iter()
            .any(|(left, right)| (left.global, right.global) == (20, 40))
    );
    assert_eq!(
        accepted_ids(&report)
            .into_iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![10, 20, 30]
    );
}

#[test]
fn detection_order_ignores_accumulated_gas() {
    let mut first = record(3, 0, 0, 1, 10);
    first.accumulated_gas = 999;
    let mut later_generation = record(4, 0, 0, 0, 1);
    later_generation.id.generation = 1;
    let mut records = BTreeSet::from([
        later_generation,
        record(1, 0, 0, 2, 1),
        record(2, 1, 0, 1, 20),
        record(5, 2, 0, 1, 10),
        first,
    ]);
    assert_eq!(
        records
            .iter()
            .map(|record| record.id.global)
            .collect::<Vec<_>>(),
        vec![4, 3, 5, 2, 1]
    );
    assert!(!records.insert(record(3, 0, 0, 1, 10)));
}

type Record = ExecutionOutput<u64, Tracked<Status, Status>>;

fn combined_records() -> Vec<Record> {
    let unsigned = CrdtValue::from(Scalar::<U256>::default());
    let signed = CrdtValue::from(
        Scalar::<I256>::new(I256::unchecked_from(-5), I256::unchecked_from(5)).unwrap(),
    );
    let mut deleter = Tracked::new_owned_existing(unsigned.clone(), 11);
    deleter.remove().unwrap();
    let mut reader = Tracked::new_owned_existing(unsigned, 30);
    assert!(reader.get().is_some());
    let mut first_delta = Tracked::new_owned_existing(signed.clone(), 5);
    first_delta
        .add_delta(CrdtDelta::I256(DeltaOp::Add(U256::from(3))))
        .unwrap();
    let mut second_delta = Tracked::new_owned_existing(signed, 20);
    second_delta
        .add_delta(CrdtDelta::I256(DeltaOp::Add(U256::from(4))))
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
        let mut detector = ConflictDetector::new();
        for record in records {
            detector.import(vec![record]);
        }
        let result = detector.check();
        assert_eq!(
            rejected_ids(&result)
                .iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![20, 30]
        );
        assert!(!result[2].accepted);
        let CommitVerdict { peers, reasons, .. } = &result[2];
        assert_eq!(
            peers.iter().map(|id| id.global).collect::<Vec<_>>(),
            vec![5]
        );
        assert!(matches!(
            reasons.as_slice(),
            [(
                Some(20),
                Reason::NumericError(NumericError::AboveUpperLimit(_))
            )]
        ));
        assert!(!result[3].accepted);
        let CommitVerdict { peers, reasons, .. } = &result[3];
        assert_eq!(
            peers.iter().map(|id| id.global).collect::<Vec<_>>(),
            vec![11]
        );
        assert!(matches!(
            reasons.as_slice(),
            [(
                Some(10),
                Reason::AccessConflict(AccessConflict::RemoveConflict(_))
            )]
        ));
    }
}

#[test]
fn verdict_helpers_preserve_full_job_ids() {
    let mut records = combined_records();
    for record in &mut records {
        record.id.block = 42;
        record.id.generation = 7;
    }
    let ids: std::collections::BTreeMap<_, _> = records
        .iter()
        .map(|record| (record.id.global, record.id))
        .collect();
    let mut detector = ConflictDetector::new();
    detector.import(records);
    let result = detector.check();
    assert_eq!(accepted_ids(&result), vec![ids[&5], ids[&11]]);
    assert_eq!(rejected_ids(&result), vec![ids[&20], ids[&30]]);
    assert!(
        result
            .iter()
            .all(|verdict| verdict.id.block == 42 && verdict.id.generation == 7)
    );
    let empty = Vec::<CommitVerdict<u64>>::new();
    assert!(accepted_ids(&empty).is_empty());
    assert!(rejected_ids(&empty).is_empty());
}

#[test]
fn preserves_access_and_numeric_reasons_for_the_same_job() {
    let value = CrdtValue::from(
        Scalar::<I256>::new(I256::unchecked_from(-5), I256::unchecked_from(5)).unwrap(),
    );
    let mut first = Tracked::new_owned_existing(value.clone(), 1);
    first
        .add_delta(CrdtDelta::I256(DeltaOp::Add(U256::from(3))))
        .unwrap();
    let mut second = Tracked::new_owned_existing(value, 2);
    assert!(second.get().is_some());
    second
        .add_delta(CrdtDelta::I256(DeltaOp::Add(U256::from(4))))
        .unwrap();
    let mut detector = ConflictDetector::new();
    detector.import(vec![transition(7, second)]);
    detector.import(vec![transition(7, first)]);
    let result = detector.check();
    assert_eq!(
        accepted_ids(&result)
            .iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![1]
    );
    assert!(!result[1].accepted);
    let CommitVerdict { peers, reasons, .. } = &result[1];
    assert_eq!(
        peers.iter().map(|id| id.global).collect::<Vec<_>>(),
        vec![1]
    );
    assert_eq!(reasons.len(), 2);
    assert!(reasons.iter().all(|(key, _)| *key == Some(7)));
    assert!(reasons.iter().any(|(_, reason)| matches!(
        reason,
        Reason::AccessConflict(AccessConflict::ReadConflict(_))
    )));
    assert!(reasons.iter().any(|(_, reason)| matches!(
        reason,
        Reason::NumericError(NumericError::AboveUpperLimit(_))
    )));
}

#[test]
fn consumes_both_stages_and_accepts_a_fresh_batch() {
    let mut detector = ConflictDetector::new();
    assert!(detector.check().is_empty());
    for _ in 0..2 {
        detector.import(combined_records());
        let result = detector.check();
        assert_eq!(
            rejected_ids(&result)
                .iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![20, 30]
        );
        assert!(detector.check().is_empty());
    }
}
#[test]
fn numeric_reads_remain_visible_to_access_detection() {
    for (value, delta) in [
        (
            CrdtValue::from(Scalar::<I256>::default()),
            CrdtDelta::I256(DeltaOp::Add(U256::from(1))),
        ),
        (
            CrdtValue::from(Scalar::<U256>::default()),
            CrdtDelta::U256(DeltaOp::Add(U256::from(1))),
        ),
        (
            CrdtValue::from(Scalar::<U256>::default()),
            CrdtDelta::U256(DeltaOp::Add(U256::from(1))),
        ),
    ] {
        let mut reader = Tracked::new_owned_existing(value.clone(), 1);
        assert!(reader.get().is_some());
        let mut writer = Tracked::new_owned_existing(value, 2);
        writer.add_delta(delta).unwrap();
        let mut detector = ConflictDetector::new();
        detector.import(vec![transition(7, writer)]);
        detector.import(vec![transition(7, reader)]);
        let result = detector.check();
        assert_eq!(
            rejected_ids(&result)
                .iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(!result[1].accepted);
        let CommitVerdict { peers, reasons, .. } = &result[1];
        assert_eq!(
            peers.iter().map(|id| id.global).collect::<Vec<_>>(),
            vec![1]
        );
        assert!(matches!(
            reasons.as_slice(),
            [(
                Some(7),
                Reason::AccessConflict(AccessConflict::ReadConflict(_))
            )]
        ));
    }
}

#[test]
fn numeric_overflow_and_underflow_reach_the_checker_for_every_numeric_type() {
    let mut u64_base = Scalar::<U256>::default();
    u64_base.add_delta(&DeltaOp::Add(U256::from(10))).unwrap();
    u64_base.apply_delta();
    let mut u256_base = Scalar::<U256>::default();
    u256_base.add_delta(&DeltaOp::Add(U256::from(10))).unwrap();
    u256_base.apply_delta();

    for (value, first_delta, second_delta, underflow) in [
        (
            CrdtValue::from(Scalar::<I256>::default()),
            CrdtDelta::I256(DeltaOp::Add(U256::from(1))),
            CrdtDelta::I256(DeltaOp::Add(U256::MAX >> 1)),
            false,
        ),
        (
            CrdtValue::from(Scalar::<I256>::default()),
            CrdtDelta::I256(DeltaOp::Sub((U256::from(1) << 255) - U256::from(1))),
            CrdtDelta::I256(DeltaOp::Sub(U256::from(2))),
            true,
        ),
        (
            CrdtValue::from(Scalar::<U256>::default()),
            CrdtDelta::U256(DeltaOp::Add(U256::from(1))),
            CrdtDelta::U256(DeltaOp::Add(U256::MAX)),
            false,
        ),
        (
            CrdtValue::from(u64_base),
            CrdtDelta::U256(DeltaOp::Sub(U256::from(6))),
            CrdtDelta::U256(DeltaOp::Sub(U256::from(5))),
            true,
        ),
        (
            CrdtValue::from(Scalar::<U256>::default()),
            CrdtDelta::U256(DeltaOp::Add(U256::from(1))),
            CrdtDelta::U256(DeltaOp::Add(U256::MAX)),
            false,
        ),
        (
            CrdtValue::from(u256_base),
            CrdtDelta::U256(DeltaOp::Sub(U256::from(6))),
            CrdtDelta::U256(DeltaOp::Sub(U256::from(5))),
            true,
        ),
    ] {
        let mut first = Tracked::new_owned_existing(value.clone(), 1);
        first.add_delta(first_delta).unwrap();
        let mut second = Tracked::new_owned_existing(value, 2);
        second.add_delta(second_delta).unwrap();
        let mut detector = ConflictDetector::new();
        detector.import(vec![transition(7, second)]);
        detector.import(vec![transition(7, first)]);
        let result = detector.check();
        assert_eq!(
            rejected_ids(&result)
                .iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(!result[1].accepted);
        let CommitVerdict { reasons, .. } = &result[1];
        assert_eq!(reasons[0].0, Some(7));
        if underflow {
            assert!(matches!(
                reasons[0].1,
                Reason::NumericError(NumericError::Underflow(_))
            ));
        } else {
            assert!(matches!(
                reasons[0].1,
                Reason::NumericError(NumericError::Overflow(_))
            ));
        }
    }
}

#[test]
fn accepts_conflict_free_jobs_across_imports() {
    let mut checker = ConflictDetector::new();
    checker.import(vec![record(1, 0, 0, 10, 5)]);
    checker.import(vec![
        record(2, 0, 1, 20, 7),
        record(2, 0, 1, 21, 7),
        record(3, 1, 0, 30, 9),
    ]);
    let result = checker.check();
    assert_eq!(
        accepted_ids(&result)
            .into_iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(result.iter().all(|verdict| verdict.accepted));
    assert!(
        rejected_ids(&result)
            .into_iter()
            .map(|id| id.global)
            .collect::<Vec<_>>()
            .is_empty()
    );
    assert!(checker.check().is_empty());
}

#[test]
fn keeps_expensive_suffixes_and_separates_generations() {
    for reverse in [false, true] {
        let mut later_generation = record(70, 0, 2, 70, 1);
        later_generation.id.generation = 1;
        let mut records = vec![
            record(50, 0, 0, 50, 5),
            record(5, 0, 1, 7, 30),
            record(1, 0, 2, 1, 1),
            record(90, 0, 3, 8, 40),
            record(100, 1, 0, 7, 20),
            record(101, 2, 0, 8, 10),
            later_generation,
        ];
        if reverse {
            records.reverse();
        }
        let mut checker = ConflictDetector::new();
        checker.import(records);
        let result = checker.check();
        assert_eq!(
            accepted_ids(&result)
                .into_iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![1, 5, 50, 70, 90]
        );
        assert_eq!(
            rejected_ids(&result)
                .into_iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![100, 101]
        );
    }
}

#[test]
fn rolling_back_one_job_preserves_its_nonconflicting_peers() {
    for reverse in [false, true] {
        let mut records = vec![
            record(1, 0, 0, 7, 5),
            record(1, 0, 0, 8, 5),
            record(2, 1, 0, 7, 10),
            record(3, 2, 0, 8, 20),
        ];
        if reverse {
            records.reverse();
        }
        let mut checker = ConflictDetector::new();
        for record in records {
            checker.import(vec![record]);
        }
        let result = checker.check();
        assert_eq!(
            accepted_ids(&result)
                .into_iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(
            pairs(&result)
                .iter()
                .map(|(a, b)| (a.global, b.global))
                .collect::<Vec<_>>(),
            vec![(1, 2), (1, 3)]
        );
        assert_eq!(
            rejected_ids(&result)
                .into_iter()
                .map(|id| id.global)
                .collect::<Vec<_>>(),
            vec![1]
        );
        assert!(!result[0].accepted);
        let CommitVerdict { peers, reasons, .. } = &result[0];
        assert_eq!(
            peers.iter().map(|id| id.global).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(
            reasons.iter().map(|(key, _)| *key).collect::<Vec<_>>(),
            vec![Some(7), Some(8)]
        );
        assert!(result[1..].iter().all(|verdict| verdict.accepted));
        assert!(checker.check().is_empty());
    }
}

#[test]
fn suffix_rejections_identify_the_earlier_rejected_job() {
    let mut records = vec![
        record(1, 0, 0, 1, 10),
        record(2, 0, 1, 7, 5),
        record(3, 0, 2, 8, 1),
        record(4, 1, 0, 7, 100),
        record(5, 2, 0, 8, 100),
    ];
    for record in &mut records {
        record.id.block = 42;
        record.id.generation = 7;
    }
    let rejected = records[1].id;
    let dependent = records[2].id;
    let peer = records[3].id;
    let dependent_peer = records[4].id;
    let mut detector = ConflictDetector::new();
    detector.import(records);
    let result = detector.check();
    assert_eq!(rejected_ids(&result), vec![rejected, dependent]);
    assert!(!result[1].accepted);
    assert_eq!(result[1].peers, vec![peer]);
    assert_eq!(result[1].reasons.len(), 1);
    assert_eq!(result[1].reasons[0].0, Some(7));
    assert!(!result[2].accepted);
    assert!(
        result[2]
            .reasons
            .contains(&(None, Reason::Dependency(rejected)))
    );
    assert_eq!(result[2].peers, vec![dependent_peer]);
    assert_eq!(result[2].reasons.len(), 2);
    assert_eq!(result[2].reasons[1].0, Some(8));
    assert_eq!(
        pairs(&result),
        BTreeSet::from([(rejected, peer), (dependent, dependent_peer)])
    );
}
