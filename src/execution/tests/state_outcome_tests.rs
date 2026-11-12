use crate::execution::{JobResult, StateOutcome, workload::ID};
use crate::planner::resolver::{CommitDecision, CommitVerdict, Reason};
use alloy_evm::revm::{
    context_interface::result::{
        ExecutionResult, Output, ResultAndState, ResultGas, SuccessReason,
    },
    state::{Account, EvmState},
};
use alloy_primitives::{Address, Bytes};

fn raw_job(id: ID) -> JobResult<()> {
    JobResult::new(
        id,
        Ok(ResultAndState::new(
            ExecutionResult::Success {
                reason: SuccessReason::Stop,
                gas: ResultGas::default(),
                logs: Vec::new(),
                output: Output::Call(Bytes::new()),
            },
            EvmState::default(),
        )),
        Some(()),
        Vec::new(),
    )
}

#[test]
fn new_job_starts_pending() {
    let id = ID::new(0, 0);
    let job = raw_job(id);
    assert_eq!(job.verdict.id, id);
    assert_eq!(job.verdict.decision, CommitDecision::Pending);
    assert!(job.verdict.conflicts.is_empty());
}

#[test]
fn pending_job_returns_an_error() {
    let id = ID::new(0, 0);
    let address = Address::with_last_byte(1);
    let mut account = Account::default().with_touched_mark();
    account.info.nonce = 1;
    let result = StateOutcome {
        raw: vec![raw_job(id)],
        committed: vec![EvmState::from_iter([(address, account)])],
    }
    .finalize(&crate::execution::StateDb::default());
    assert!(matches!(result, Err(error) if error.to_string().contains("pending verdict")));
}

#[test]
fn finalization_filters_committed_job_states_without_rebuilding_from_raw() {
    let mut raw = Vec::new();
    let mut states = Vec::new();
    let ids = [ID::new(0, 0), ID::new(0, 1), ID::new(0, 2)];
    for (index, id) in ids.into_iter().enumerate() {
        raw.push(raw_job(id));
        let state = EvmState::from_iter([(Address::with_last_byte(index as u8 + 1), {
            let mut account = Account::default().with_touched_mark();
            account.info.nonce = index as u64 + 1;
            account
        })]);
        states.push(state);
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
    let mut outcome = StateOutcome {
        raw,
        committed: states,
    }
    .finalize(&crate::execution::StateDb::default())
    .unwrap();

    assert_eq!(outcome.committed.len(), 1);
    assert_eq!(outcome.committed[&Address::with_last_byte(1)].info.nonce, 1);
    assert_eq!(outcome.raw.len(), 3);
    for (raw, verdict) in outcome.raw.iter_mut().zip(&verdicts) {
        assert_eq!(&raw.verdict, verdict);
        assert!(raw.result_and_state().as_ref().unwrap().state.is_empty());
    }
}
