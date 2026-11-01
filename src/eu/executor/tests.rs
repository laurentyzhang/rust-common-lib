use super::*;
use crate::{
    crdt::{
        Crdt,
        state::{DeltaOp, Status, status::Tag, value::CrdtValue},
    },
    eu::{ApiAddress, Field, KeyCodec, ScalarAdapter, StatDbCommitter},
    planner::resolver::{
        ConflictChecker,
        reason::{AccessConflict, NumericError, Reason},
    },
};
use alloy_evm::revm::{
    context_interface::result::{
        ExecutionResult, Output, ResultAndState, ResultGas, SuccessReason,
    },
    state::EvmStorageSlot,
};
use alloy_primitives::{Bytes, U256};

fn successful_job(id: u64, gas: u64, state: EvmState) -> JobOutcome<u64> {
    JobOutcome::new(
        ID::new(id),
        Ok(ResultAndState::new(
            ExecutionResult::Success {
                reason: SuccessReason::Stop,
                gas: ResultGas::default().with_total_gas_spent(gas),
                logs: Vec::new(),
                output: Output::Call(Bytes::new()),
            },
            state,
        )),
        Some(0),
        Vec::new(),
    )
}

fn scalar_state(original: [u64; 5], current: [u64; 5]) -> EvmState {
    let mut account = Account::default().with_touched_mark();
    let slots = ScalarAdapter::new(KeyCodec::encode(
        ApiAddress::Scalar,
        Field::Value as u8,
        Address::ZERO,
    ));
    for ((key, original), current) in [
        slots.min,
        slots.max,
        slots.value,
        slots.pos_delta,
        slots.neg_delta,
    ]
    .into_iter()
    .zip(original)
    .zip(current)
    {
        account.storage.insert(
            key,
            EvmStorageSlot::new_changed(
                U256::from(original),
                U256::from(current),
                Default::default(),
            ),
        );
    }
    EvmState::from_iter([(Address::ZERO, account)])
}

#[test]
fn scalar_exports_preserve_snapshots_and_combine_reads_without_stripping_numeric_data() {
    let original = [0, 100, 42, 5, 0];
    let current = [0, 100, 42, 0, 0];
    let slots = ScalarAdapter::new(KeyCodec::encode(
        ApiAddress::Scalar,
        Field::Value as u8,
        Address::ZERO,
    ));
    let mut outcome = successful_job(7, 21_000, scalar_state(original, current));
    let mut reads = ReadRecorder::new(7);
    for key in [slots.value, slots.pos_delta, slots.neg_delta] {
        reads.record(Key::Storage(Address::ZERO, key));
    }
    outcome.reads = reads.drain();
    outcome
        .result_and_state
        .as_mut()
        .unwrap()
        .state
        .get_mut(&Address::ZERO)
        .unwrap()
        .storage
        .insert(
            U256::from(7),
            EvmStorageSlot::new_changed(U256::from(10), U256::from(11), Default::default()),
        );
    let records = outcome.merge_records(outcome.id);
    assert_eq!(records.len(), 3); // Account, scalar, and ordinary storage slot.
    let scalar = &records
        .iter()
        .find(|(key, _)| *key == Key::Storage(Address::ZERO, slots.value))
        .unwrap()
        .1;
    assert_eq!(scalar.operations.reads.count(), 3);
    assert_eq!(scalar.operations.deltas.count(), 1);
    assert_eq!(scalar.operations.removes.count(), 0);
    for (snapshot, expected) in [(scalar.original(), original), (scalar.current(), current)] {
        let Status::Value(CrdtValue::U256(value)) = snapshot else {
            panic!("scalar snapshots must retain their numeric payload");
        };
        assert_eq!(
            (value.min, value.max, value.value),
            (
                U256::from(expected[0]),
                U256::from(expected[1]),
                U256::from(expected[2])
            )
        );
        assert_eq!(
            value.delta().copied(),
            if expected[3] == 0 {
                None
            } else {
                Some(DeltaOp::Add(U256::from(expected[3])))
            }
        );
    }
    let ordinary = &records
        .iter()
        .find(|(key, _)| *key == Key::Storage(Address::ZERO, U256::from(7)))
        .unwrap()
        .1;
    assert!(matches!(ordinary.current(), Status::Tag(Tag::Stripped)));
}

#[test]
fn zero_scalar_reads_keep_numeric_payloads_and_do_not_conflict() {
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let mut results = (0..2)
        .map(|id| {
            let mut job = successful_job(id, 21_000, scalar_state([0; 5], [0; 5]));
            let mut reads = ReadRecorder::new(id);
            reads.record(Key::Storage(Address::ZERO, key));
            let outcome = &mut job;
            outcome.reads = reads.drain();
            assert!(
                outcome
                    .merge_records(outcome.id)
                    .iter()
                    .any(|(record_key, data)| {
                        *record_key == Key::Storage(Address::ZERO, key)
                            && matches!(
                                data.current(),
                                Status::Value(CrdtValue::I256(_) | CrdtValue::U256(_))
                            )
                    })
            );
            job
        })
        .collect::<Vec<_>>();
    ArcologyParaBlockExecutor::resolve_generation(
        &mut results,
        &mut ConflictChecker::new(),
        &mut StatDbCommitter::new(),
    );
    assert!(results.iter().all(|result| result.result_and_state.is_ok()));
}

#[test]
fn scalar_creations_with_different_bounds_conflict() {
    let mut results = vec![
        successful_job(0, 21_000, scalar_state([0; 5], [0, 10, 0, 0, 0])),
        successful_job(1, 21_000, scalar_state([0; 5], [0, 20, 0, 0, 0])),
    ];
    ArcologyParaBlockExecutor::resolve_generation(
        &mut results,
        &mut ConflictChecker::new(),
        &mut StatDbCommitter::new(),
    );
    assert!(results[0].result_and_state.is_ok());
    assert!(
        matches!(&results[1].result_and_state, Err(JobError::Rejected(conflicts))
        if conflicts.iter().any(|(_, reason)| matches!(reason, Reason::AccessConflict(AccessConflict::CreationConflict(_)))))
    );
}

#[test]
fn scalar_bounds_reach_the_resolver_without_counting_existing_deltas_twice() {
    for (max, rejected, expected_delta, expected_gas) in
        [(55, true, 9, 21_000), (60, false, 14, 42_000)]
    {
        let original = [0, max, 42, 5, 0];
        let mut results = vec![
            successful_job(0, 21_000, scalar_state(original, [0, max, 42, 9, 0])),
            successful_job(1, 21_000, scalar_state(original, [0, max, 42, 10, 0])),
        ];
        let mut accumulator = StatDbCommitter::with_state(scalar_state(original, original));
        ArcologyParaBlockExecutor::resolve_generation(
            &mut results,
            &mut ConflictChecker::new(),
            &mut accumulator,
        );
        assert!(results[0].result_and_state.is_ok());
        if rejected {
            assert!(
                matches!(&results[1].result_and_state, Err(JobError::Rejected(conflicts))
                if conflicts.iter().any(|(_, reason)| matches!(reason, Reason::NumericError(NumericError::AboveUpperLimit(_)))))
            );
        } else {
            assert!(results[1].result_and_state.is_ok());
        }
        let delta = KeyCodec::encode(ApiAddress::Scalar, Field::PosDelta as u8, Address::ZERO);
        assert_eq!(
            accumulator.inner()[&Address::ZERO].storage[&delta].present_value(),
            U256::from(expected_delta)
        );
        assert_eq!(
            ArcologyParaBlockExecutor::accumulate_gas(&mut results, &|receipt, gas| *receipt = gas),
            expected_gas
        );
    }
}

#[test]
fn scalar_subtractions_are_checked_against_the_combined_lower_bound() {
    let original = [5, 100, 10, 0, 2];
    let mut results = vec![
        successful_job(0, 21_000, scalar_state(original, [5, 100, 10, 0, 4])),
        successful_job(1, 21_000, scalar_state(original, [5, 100, 10, 0, 5])),
    ];
    ArcologyParaBlockExecutor::resolve_generation(
        &mut results,
        &mut ConflictChecker::new(),
        &mut StatDbCommitter::new(),
    );
    assert!(results.iter().any(|result| matches!(&result.result_and_state, Err(JobError::Rejected(conflicts))
        if conflicts.iter().any(|(_, reason)| matches!(reason, Reason::NumericError(NumericError::BelowLowerLimit(_)))))));
    assert_eq!(
        results
            .iter()
            .filter(|result| result.result_and_state.is_ok())
            .count(),
        1
    );
}

#[test]
fn rejected_jobs_report_all_conflicts_and_contribute_no_state_or_gas() {
    let slots = [U256::from(1), U256::from(2)];
    let mut account = Account::default().with_touched_mark();
    for slot in slots {
        account.storage.insert(
            slot,
            EvmStorageSlot::new_changed(U256::from(10), U256::from(10), Default::default()),
        );
    }
    let state = EvmState::from_iter([(Address::ZERO, account)]);
    let mut reader = successful_job(0, 21_000, state.clone());
    let mut reads = ReadRecorder::new(0);
    for slot in slots {
        reads.record(Key::Storage(Address::ZERO, slot));
    }
    reader.reads = reads.drain();

    let mut writer = successful_job(1, 50_000, state.clone());
    for slot in slots {
        writer
            .result_and_state
            .as_mut()
            .unwrap()
            .state
            .get_mut(&Address::ZERO)
            .unwrap()
            .storage
            .get_mut(&slot)
            .unwrap()
            .present_value = U256::from(99);
    }
    let mut results = vec![writer, reader];
    let mut resolver = ConflictChecker::new();
    let mut accumulator = StatDbCommitter::with_state(state);
    ArcologyParaBlockExecutor::resolve_generation(&mut results, &mut resolver, &mut accumulator);
    let results = ArcologyParaBlockExecutor::finish_block(results, &|receipt, gas| *receipt = gas);

    assert_eq!(results[0].id.global, 0);
    assert_eq!(results[0].receipt, Some(21_000));
    let Err(JobError::Rejected(conflicts)) = &results[1].result_and_state else {
        panic!("conflicting writer must be returned as rejected");
    };
    assert_eq!(results[1].id.global, 1);
    assert_eq!(conflicts.len(), 2);
    for slot in slots {
        assert!(conflicts.iter().any(|(key, reason)| {
            *key == Key::Storage(Address::ZERO, slot)
                && matches!(
                    reason,
                    Reason::AccessConflict(AccessConflict::ReadConflict(_))
                )
        }));
        assert_eq!(
            accumulator.inner()[&Address::ZERO].storage[&slot].present_value(),
            U256::from(10)
        );
    }
    let mut results = results;
    assert_eq!(
        ArcologyParaBlockExecutor::accumulate_gas(&mut results, &|receipt, gas| *receipt = gas),
        21_000
    );
}

#[test]
fn accepted_reverts_consume_gas_and_execution_errors_stay_distinct() {
    let mut reverted = successful_job(2, 30_000, EvmState::default());
    reverted.result_and_state.as_mut().unwrap().result = ExecutionResult::Revert {
        gas: ResultGas::default().with_total_gas_spent(30_000),
        logs: Vec::new(),
        output: Bytes::new(),
    };
    let mut results = vec![
        reverted,
        JobOutcome {
            id: ID::new(1),
            result_and_state: Err(BlockExecutionError::msg("execution failed")),
            receipt: None,
            reads: Vec::new(),
        },
        successful_job(0, 21_000, EvmState::default()),
    ];
    ArcologyParaBlockExecutor::resolve_generation(
        &mut results,
        &mut ConflictChecker::new(),
        &mut StatDbCommitter::new(),
    );
    let results = ArcologyParaBlockExecutor::finish_block(results, &|receipt, gas| *receipt = gas);
    assert_eq!(results[0].receipt, Some(21_000));
    assert!(
        matches!(&results[1].result_and_state, Err(error) if error.to_string() == "execution failed")
    );
    assert_eq!(results[2].receipt, Some(51_000));
}
