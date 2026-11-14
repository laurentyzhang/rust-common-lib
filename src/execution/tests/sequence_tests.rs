use crate::execution::{
    JobResult,
    workload::{ID, JobSequence},
};
use crate::planner::resolver::{CommitDecision, CommitVerdict, Reason};
use alloy_evm::revm::{
    context_interface::result::{
        ExecutionResult, Output, ResultAndState, ResultGas, SuccessReason,
    },
    state::{Account, AccountInfo, Bytecode, EvmState, EvmStorageSlot},
};
use alloy_primitives::{Address, Bytes, U256};

fn raw_job(id: ID, state: EvmState) -> JobResult<()> {
    JobResult::new(
        id,
        Ok(ResultAndState::new(
            ExecutionResult::Success {
                reason: SuccessReason::Stop,
                gas: ResultGas::default(),
                logs: Vec::new(),
                output: Output::Call(Bytes::new()),
            },
            state,
        )),
        Some(()),
        Vec::new(),
    )
}

#[test]
fn new_job_starts_pending() {
    let id = ID::new(0, 0);
    let job = raw_job(id, EvmState::default());
    assert_eq!(job.verdict.id, id);
    assert_eq!(job.verdict.decision, CommitDecision::Pending);
    assert!(job.verdict.conflicts.is_empty());
}

#[test]
fn pending_job_returns_an_error() {
    let id = ID::new(0, 0);
    let result = JobSequence::<()>::rebuild_sequence_state(
        &mut [raw_job(id, EvmState::default())],
        &crate::execution::StateDb::default(),
    );
    assert!(matches!(result, Err(error) if error.to_string().contains("pending verdict")));
}

#[test]
fn finalization_rebuilds_from_raw_when_jobs_are_rejected() {
    let mut raw = Vec::new();
    let ids = [ID::new(0, 0), ID::new(0, 1), ID::new(0, 2)];
    for (index, id) in ids.into_iter().enumerate() {
        let changes = EvmState::from_iter([(Address::with_last_byte(index as u8 + 1), {
            let mut account = Account::default().with_touched_mark();
            account.info.nonce = index as u64 + 1;
            account
        })]);
        raw.push(raw_job(id, changes.clone()));
    }
    let verdicts = [
        CommitVerdict {
            id: ids[0],
            decision: CommitDecision::Accepted,
            conflicts: Vec::new(),
        },
        CommitVerdict {
            id: ids[1],
            decision: CommitDecision::Rejected,
            conflicts: vec![(vec![ID::new(0, 3)], Reason::None)],
        },
        CommitVerdict {
            id: ids[2],
            decision: CommitDecision::Rejected,
            conflicts: vec![(Vec::new(), Reason::Dependency(ids[1]))],
        },
    ];
    for (raw, verdict) in raw.iter_mut().zip(&verdicts) {
        raw.verdict = verdict.clone();
    }
    let committed =
        JobSequence::<()>::rebuild_sequence_state(&mut raw, &crate::execution::StateDb::default())
            .unwrap();

    assert_eq!(committed.len(), 1);
    assert_eq!(committed[&Address::with_last_byte(1)].info.nonce, 1);
    assert_eq!(raw.len(), 3);
    for (raw, verdict) in raw.iter_mut().zip(&verdicts) {
        assert_eq!(&raw.verdict, verdict);
        assert_eq!(
            raw.result_and_state().as_ref().unwrap().state.len(),
            usize::from(verdict.decision == CommitDecision::Accepted),
        );
    }
}

#[test]
fn finalization_applies_accepted_job_transitions() {
    let address = Address::with_last_byte(1);
    let mut account = Account::default().with_touched_mark();
    account.info.nonce = 2;
    let state = EvmState::from_iter([(address, account)]);
    let mut job = raw_job(ID::new(0, 0), state.clone());
    job.verdict.decision = CommitDecision::Accepted;
    let committed = JobSequence::<()>::rebuild_sequence_state(
        &mut [job],
        &crate::execution::StateDb::default(),
    )
    .unwrap();
    assert_eq!(committed, state);
}

#[test]
fn pending_verdict_is_rejected_before_rebuilding() {
    let mut account = Account::default().with_touched_mark();
    account.original_info_mut().nonce = 1;
    let mut accepted = raw_job(
        ID::new(0, 0),
        EvmState::from_iter([(Address::with_last_byte(1), account)]),
    );
    accepted.verdict.decision = CommitDecision::Accepted;
    let pending = raw_job(ID::new(0, 1), EvmState::default());
    let result = JobSequence::<()>::rebuild_sequence_state(
        &mut [accepted, pending],
        &crate::execution::StateDb::default(),
    );
    assert!(matches!(result, Err(error) if error.to_string().contains("pending verdict")));
}

#[test]
fn sequence_merges_api_transitions_after_a_rejected_job() {
    use crate::execution::{Property, ScalarKeyCodec, StateDb};
    use crate::namespace::Namespace;
    use alloy_evm::revm::state::EvmStorageSlot;

    for property in [Property::PosDelta, Property::NegDelta] {
        let key = ScalarKeyCodec::encode(Namespace::U256, property as u8, Address::ZERO);
        let state = |original: u64, present: u64| {
            let mut account = Account::default().with_touched_mark();
            account.storage.insert(
                key,
                EvmStorageSlot::new_changed(
                    U256::from(original),
                    U256::from(present),
                    Default::default(),
                ),
            );
            EvmState::from_iter([(Address::ZERO, account)])
        };
        let fallback = StateDb::new(state(10, 10));
        let mut rejected = raw_job(ID::new(0, 0), state(10, 12));
        rejected.verdict.decision = CommitDecision::Rejected;
        let mut accepted = raw_job(ID::new(0, 1), state(12, 15));
        accepted.verdict.decision = CommitDecision::Accepted;
        let committed =
            JobSequence::<()>::rebuild_sequence_state(&mut [rejected, accepted], &fallback)
                .unwrap();
        let slot = &committed[&Address::ZERO].storage[&key];
        assert_eq!(slot.original_value(), U256::from(10));
        assert_eq!(slot.present_value(), U256::from(13));
    }
}

#[test]
fn sequence_rebases_nonce_deltas_for_missing_senders() {
    use crate::execution::StateDb;
    use alloy_evm::revm::state::AccountInfo;

    for decision in [CommitDecision::Accepted, CommitDecision::Rejected] {
        let mut account = Account::from(AccountInfo::default().with_nonce(2)).with_touched_mark();
        account.info.nonce = 3;
        let state = EvmState::from_iter([(Address::ZERO, account)]);
        let mut job = raw_job(ID::new(0, 0), state.clone());
        job.verdict.decision = decision;
        let committed =
            JobSequence::<()>::rebuild_sequence_state(&mut [job], &StateDb::default()).unwrap();
        assert_eq!(committed[&Address::ZERO].original_info().nonce, 0);
        assert_eq!(committed[&Address::ZERO].info.nonce, 1);
    }
}

#[test]
fn sequence_rebases_accounts_and_preserves_code_and_storage() {
    use crate::execution::StateDb;

    let code = Bytecode::new_raw(vec![0x00].into());
    let info = AccountInfo {
        balance: U256::from(100),
        nonce: 7,
        code_hash: code.hash_slow(),
        code: Some(code.clone()),
        ..Default::default()
    };
    let mut account = Account::from(info);
    let key = U256::from(7);
    account
        .storage
        .insert(key, EvmStorageSlot::new(U256::from(10), Default::default()));
    let baseline = EvmState::from_iter([(Address::ZERO, account)]);
    let fallback = StateDb::new(baseline.clone());
    let mut incoming = Account::from(
        AccountInfo::default()
            .with_balance(U256::from(200))
            .with_nonce(8),
    )
    .with_touched_mark();
    incoming.info.balance = U256::from(190);
    incoming.info.nonce = 9;
    incoming.storage.insert(
        key,
        EvmStorageSlot::new_changed(U256::from(10), U256::from(12), Default::default()),
    );
    let mut job = raw_job(
        ID::new(0, 0),
        EvmState::from_iter([(Address::ZERO, incoming)]),
    );
    job.verdict.decision = CommitDecision::Accepted;
    let mut jobs = vec![job];
    let committed = JobSequence::<()>::rebuild_sequence_state(&mut jobs, &fallback).unwrap();
    let account = &committed[&Address::ZERO];
    assert_eq!(account.info.balance, U256::from(90));
    assert_eq!(account.info.nonce, 8);
    assert_eq!(account.original_info().balance, U256::from(100));
    assert_eq!(account.original_info().nonce, 7);
    assert_eq!(account.info.code_hash, code.hash_slow());
    assert_eq!(account.storage[&key].original_value(), U256::from(10));
    assert_eq!(account.storage[&key].present_value(), U256::from(12));

    let replacement = Bytecode::new_raw(vec![0x60, 0x01, 0x00].into());
    let mut incoming = Account::from(account.info.clone()).with_touched_mark();
    incoming.info.code_hash = replacement.hash_slow();
    incoming.info.code = Some(replacement.clone());
    incoming.storage.insert(
        key,
        EvmStorageSlot::new_changed(U256::from(12), U256::from(15), Default::default()),
    );
    let mut job = raw_job(
        ID::new(0, 1),
        EvmState::from_iter([(Address::ZERO, incoming)]),
    );
    job.verdict.decision = CommitDecision::Accepted;
    jobs.push(job);
    let committed = JobSequence::<()>::rebuild_sequence_state(&mut jobs, &fallback).unwrap();
    let account = &committed[&Address::ZERO];
    assert_eq!(account.info.code, Some(replacement));
    assert_eq!(account.storage[&key].original_value(), U256::from(10));
    assert_eq!(account.storage[&key].present_value(), U256::from(15));
}

#[test]
fn invalid_sequence_transitions_leave_the_fallback_unchanged() {
    use crate::execution::StateDb;

    let fallback = StateDb::new(EvmState::from_iter([(Address::ZERO, Account::default())]));
    let original = fallback.inner().clone();
    let mut incoming =
        Account::from(AccountInfo::default().with_balance(U256::from(10))).with_touched_mark();
    incoming.info.balance = U256::from(5);
    let mut job = raw_job(
        ID::new(0, 0),
        EvmState::from_iter([(Address::ZERO, incoming)]),
    );
    job.verdict.decision = CommitDecision::Accepted;
    assert!(JobSequence::<()>::rebuild_sequence_state(&mut [job], &fallback).is_err());
    assert_eq!(fallback.inner(), &original);
}

#[test]
fn sequence_creation_resets_api_storage() {
    use crate::execution::{Property, ScalarKeyCodec, StateDb};
    use crate::namespace::Namespace;
    let key = ScalarKeyCodec::encode(Namespace::U256, Property::PosDelta as u8, Address::ZERO);
    let mut account = Account::default();
    account
        .storage
        .insert(key, EvmStorageSlot::new(U256::from(10), Default::default()));
    let fallback = StateDb::new(EvmState::from_iter([(Address::ZERO, account)]));
    let mut incoming = Account::default().with_touched_mark();
    incoming.mark_created();
    incoming.info.nonce = 1;
    incoming.storage.insert(
        key,
        EvmStorageSlot::new_changed(U256::ZERO, U256::from(3), Default::default()),
    );
    let mut job = raw_job(
        ID::new(0, 0),
        EvmState::from_iter([(Address::ZERO, incoming)]),
    );
    job.verdict.decision = CommitDecision::Accepted;
    let committed = JobSequence::<()>::rebuild_sequence_state(&mut [job], &fallback).unwrap();
    let account = &committed[&Address::ZERO];
    assert!(account.is_created());
    assert_eq!(account.storage[&key].original_value(), U256::ZERO);
    assert_eq!(account.storage[&key].present_value(), U256::from(3));
}
