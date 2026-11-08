use crate::execution::{
    EvmStateCommitter, JobResult, Property, ScalarKeyCodec, StateOutcome, workload::ID,
};
use crate::namespace::Namespace;
use crate::planner::resolver::{CommitVerdict, Reason};
use crate::store::traits::Committer;
use alloy_evm::revm::{
    context_interface::result::{
        ExecutionResult, Output, ResultAndState, ResultGas, SuccessReason,
    },
    state::{Account, AccountInfo, EvmState, EvmStorageSlot},
};
use alloy_primitives::{Address, Bytes, U256};

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
        let state = EvmState::from_iter([(
            Address::with_last_byte(index as u8 + 1),
            Account::from(AccountInfo {
                nonce: index as u64 + 1,
                ..Default::default()
            })
            .with_touched_mark(),
        )]);
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
    .finalize();

    assert_eq!(outcome.committed.len(), 1);
    assert_eq!(outcome.committed[&Address::with_last_byte(1)].info.nonce, 1);
    assert_eq!(outcome.raw.len(), 3);
    for (raw, verdict) in outcome.raw.iter().zip(&verdicts) {
        assert_eq!(raw.verdict.as_ref(), Some(verdict));
        assert!(raw.result_and_state.as_ref().unwrap().state.is_empty());
    }
}

#[test]
fn finalized_sequence_states_preserve_serial_and_parallel_delta_contributions() {
    let key = ScalarKeyCodec::encode(Namespace::U256, Property::PosDelta as u8, Address::ZERO);
    let mut generation = EvmStateCommitter::new();
    for (sequence, changes) in [vec![(5, 8), (8, 10)], vec![(5, 9)]]
        .into_iter()
        .enumerate()
    {
        let mut raw = Vec::new();
        let mut committer = EvmStateCommitter::new();
        for (job, (original, present)) in changes.into_iter().enumerate() {
            let id = ID {
                sequence: sequence as u64,
                job: job as u64,
                ..ID::new(0, (sequence * 2 + job) as u64)
            };
            let mut account = Account::default().with_touched_mark();
            account.storage.insert(
                key,
                EvmStorageSlot::new_changed(
                    U256::from(original),
                    U256::from(present),
                    Default::default(),
                ),
            );
            let state = EvmState::from_iter([(Address::ZERO, account)]);
            committer.buffer(id, state);
            let mut job = raw_job(id);
            job.verdict = Some(CommitVerdict {
                id,
                accepted: true,
                peers: Vec::new(),
                reasons: Vec::new(),
            });
            raw.push(job);
        }
        let outcome = StateOutcome {
            raw,
            committed: committer,
        }
        .finalize();
        generation.buffer(outcome.raw[0].id, outcome.committed);
    }
    generation.flush();
    let state = generation.into_inner();
    let delta = &state[&Address::ZERO].storage[&key];
    assert_eq!(delta.original_value(), U256::from(5));
    assert_eq!(delta.present_value(), U256::from(14));
}
