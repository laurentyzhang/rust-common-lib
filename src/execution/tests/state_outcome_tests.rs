use crate::execution::{EvmStateCommitter, JobResult, StateOutcome, workload::ID};
use crate::planner::resolver::{CommitVerdict, Reason};
use crate::store::traits::Committer;
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
fn finalization_filters_committed_job_states_without_rebuilding_from_raw() {
    let mut raw = Vec::new();
    let mut committer = EvmStateCommitter::new();
    let ids = [ID::new(0, 0), ID::new(0, 1), ID::new(0, 2)];
    for (index, id) in ids.into_iter().enumerate() {
        raw.push(raw_job(id));
        let state = EvmState::from_iter([(Address::with_last_byte(index as u8 + 1), {
            let mut account = Account::default().with_touched_mark();
            account.info.nonce = index as u64 + 1;
            account
        })]);
        committer.buffer(id, state);
    }
    let verdicts = [
        CommitVerdict {
            id: ids[0],
            accepted: true,
            peers: Vec::new(),
            reasons: Vec::new(),
        },
        CommitVerdict {
            id: ids[1],
            accepted: false,
            peers: vec![ID::new(0, 3)],
            reasons: Vec::new(),
        },
        CommitVerdict {
            id: ids[2],
            accepted: false,
            peers: Vec::new(),
            reasons: vec![(None, Reason::Dependency(ids[1]))],
        },
    ];
    for (raw, verdict) in raw.iter_mut().zip(&verdicts) {
        raw.verdict = Some(verdict.clone());
    }
    let outcome = StateOutcome {
        raw,
        committed: committer,
    }
    .finalize(&crate::execution::StateDb::default())
    .unwrap();

    assert_eq!(outcome.committed.len(), 1);
    assert_eq!(outcome.committed[&Address::with_last_byte(1)].info.nonce, 1);
    assert_eq!(outcome.raw.len(), 3);
    for (raw, verdict) in outcome.raw.iter().zip(&verdicts) {
        assert_eq!(raw.verdict.as_ref(), Some(verdict));
        assert!(raw.result_and_state.as_ref().unwrap().state.is_empty());
    }
}
