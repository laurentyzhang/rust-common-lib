use super::*;
use crate::eu::{ArcologyBlockExecutorFactory, ArcologyTx, CallRouter, StatDbCommitter, StateDb};
use crate::planner::resolver::ConflictChecker;
use alloy_evm::{
    FromRecoveredTx, RecoveredTx,
    revm::{
        Database, DatabaseRef,
        state::{Account, AccountInfo, Bytecode, EvmState},
    },
};
use alloy_op_evm::block::OpAlloyReceiptBuilder;
use alloy_op_hardforks::OpChainHardforks;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use op_alloy::consensus::{OpTxEnvelope, TxDeposit};
use op_revm::OpSpecId;
use std::collections::BTreeSet;
use std::sync::Arc;

struct Recovered {
    tx: OpTxEnvelope,
    signer: Address,
}

impl RecoveredTx<OpTxEnvelope> for Recovered {
    fn tx(&self) -> &OpTxEnvelope {
        &self.tx
    }
    fn signer(&self) -> &Address {
        &self.signer
    }
}

fn job(id: u64, contract: Address) -> Job<(ArcologyTx, Recovered)> {
    let caller = Address::with_last_byte(id as u8 + 0x10);
    let tx = OpTxEnvelope::from(TxDeposit {
        from: caller,
        to: TxKind::Call(contract),
        gas_limit: 100_000,
        ..Default::default()
    });
    let env = ArcologyTx::from_recovered_tx(&tx, caller);
    Job::new(
        ID {
            sequence: id,
            ..ID::new(id)
        },
        (env, Recovered { tx, signer: caller }),
    )
}

#[test]
fn sequence_continues_after_rejected_job_with_prior_state() {
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::REGOLITH;
    env.block_env.timestamp = U256::from(1_700_000_000u64);
    env.block_env.gas_limit = 200_000;
    let contract = Address::repeat_byte(0x99);
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0x00, 0x54, 0x60, 0x01, 0x01, 0x60, 0x00, 0x55, 0x00,
    ]));
    let fallback = StateDb::new(EvmState::from_iter([(
        contract,
        Account::from(AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        }),
    )]));
    let caller = Address::repeat_byte(0x22);
    let tx = OpTxEnvelope::from(TxDeposit {
        from: caller,
        to: TxKind::Call(contract),
        gas_limit: 200_001,
        ..Default::default()
    });
    let rejected = Job::new(
        ID::new(2),
        (
            ArcologyTx::from_recovered_tx(&tx, caller),
            Recovered { tx, signer: caller },
        ),
    );
    let mut outcomes = JobSequence {
        jobs: vec![job(1, contract), rejected, job(3, contract)],
        nonce_offsets: Vec::new(),
    }
    .run(&factory, &fallback, env, OpBlockExecutionCtx::default())
    .unwrap();

    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| outcome.id.global)
            .collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(outcomes[1].result_and_state.is_err());
    assert!(outcomes[1].receipt.is_none());
    assert!(outcomes[1].merge_records(outcomes[1].id).is_empty());
    for (index, outcome) in outcomes
        .iter()
        .filter_map(|outcome| outcome.result_and_state.as_ref().ok())
        .enumerate()
    {
        assert!(outcome.result.is_success());
        assert_eq!(
            outcome.state[&contract].storage[&U256::ZERO].present_value(),
            U256::from(index + 1),
        );
    }
    assert_eq!(
        fallback.storage_ref(contract, U256::ZERO).unwrap(),
        U256::ZERO
    );
    assert!(fallback.basic_ref(caller).unwrap().is_none());

    let gas_used: u64 = outcomes
        .iter()
        .filter_map(|outcome| outcome.result_and_state.as_ref().ok())
        .map(|executed| executed.result.tx_gas_used())
        .sum();
    outcomes.reverse();
    let cumulative = std::cell::Cell::new(0);
    let outcomes = ArcologyParaBlockExecutor::finish_block(outcomes, &|_, gas| cumulative.set(gas));
    assert_eq!(outcomes[0].id.global, 1);
    assert_eq!(outcomes[1].id.global, 2);
    assert_eq!(outcomes[2].id.global, 3);
    assert!(outcomes[1].result_and_state.is_err());
    assert_eq!(cumulative.get(), gas_used);
}

#[test]
fn sequence_nonce_offsets_are_local_and_checked() {
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let mut env = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::BEDROCK;
    for (nonce, offset, expected) in [(7, 0, Some(8)), (7, 2, Some(10)), (u64::MAX, 1, None)] {
        let job = job(1, Address::repeat_byte(0x99));
        let caller = Address::from(job.sender);
        let fallback = StateDb::new(EvmState::from_iter([(
            caller,
            Account::from(AccountInfo {
                nonce,
                ..Default::default()
            }),
        )]));
        let mut sequence = JobSequence {
            jobs: vec![job],
            nonce_offsets: vec![Default::default()],
        };
        sequence.nonce_offsets[0].sender = caller.into_array();
        sequence.nonce_offsets[0].offset = offset;
        let result = sequence.run(
            &factory,
            &fallback,
            env.clone(),
            OpBlockExecutionCtx::default(),
        );
        if let Some(expected) = expected {
            let outcomes = result.unwrap();
            let executed = outcomes[0].result_and_state.as_ref().unwrap();
            assert!(executed.result.is_success());
            assert_eq!(executed.state[&caller].info.nonce, expected);
        } else {
            let error = result
                .err()
                .expect("nonce overflow must reject the sequence");
            assert!(error.to_string().contains("sequence sender nonce overflow"));
        }
        assert_eq!(fallback.basic_ref(caller).unwrap().unwrap().nonce, nonce);
    }
}

#[test]
fn detects_conflicts_in_serial_and_parallel_generations_without_reusing_records() {
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let mut env = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::BEDROCK;
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let contract = Address::repeat_byte(0x99);
    // Read slot zero, increment it, then write it back.
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0x00, 0x54, 0x60, 0x01, 0x01, 0x60, 0x00, 0x55, 0x00,
    ]));
    let state = EvmState::from_iter([(
        contract,
        Account::from(AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        }),
    )]);

    let persistent = Arc::new(StateDb::new(state.clone()));
    for workers in [1, 2] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        let mut checker = ConflictChecker::new();
        let mut committer = StatDbCommitter::with_state(state.clone());
        let generation = Generation {
            sequences: [1, 2]
                .into_iter()
                .map(|id| JobSequence {
                    jobs: vec![job(id, contract)],
                    nonce_offsets: Vec::new(),
                })
                .collect(),
        };
        let (outcomes, result) = ArcologyParaBlockExecutor::run_generation(
            &factory,
            &config,
            &pool,
            &persistent,
            generation,
            &mut checker,
            &mut committer,
        )
        .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes.iter().all(|result| {
            result
                .result_and_state
                .as_ref()
                .unwrap()
                .result
                .is_success()
        }));
        assert!(result.committable.is_empty());
        assert_eq!(
            result
                .conflicts
                .iter()
                .map(|(left, right)| (left.global, right.global))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(1, 2)])
        );
        assert_eq!(committer.inner(), &state);
        assert!(
            !committer
                .inner()
                .contains_key(&Address::with_last_byte(0x12))
        );
        assert!(
            result
                .violations
                .iter()
                .any(|(record, _)| record.id.global == 2
                    && record.key == Key::Storage(contract, U256::ZERO))
        );
        assert!(
            result
                .violations
                .iter()
                .all(|(record, _)| record.gas_used > 0)
        );

        let generation = Generation {
            sequences: vec![JobSequence {
                jobs: vec![job(3, contract)],
                nonce_offsets: Vec::new(),
            }],
        };
        let fallback = Arc::new(StateDb::new(committer.inner().clone()));
        let (outcomes, result) = ArcologyParaBlockExecutor::run_generation(
            &factory,
            &config,
            &pool,
            &fallback,
            generation,
            &mut checker,
            &mut committer,
        )
        .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(result.committable.is_empty());
        assert!(result.violations.is_empty());
        assert!(result.conflicts.is_empty());
        assert_eq!(
            outcomes[0].result_and_state.as_ref().unwrap().state[&contract].storage[&U256::ZERO]
                .present_value(),
            U256::from(1)
        );
        assert_eq!(committer.inner(), &state);

        let mut planner = Planner::new(StateDb::new(state.clone()), Default::default());
        let mut committer = StatDbCommitter::with_state(state.clone());
        let (outcomes, resolutions) = ArcologyParaBlockExecutor::execute_block(
            &factory,
            &config,
            &pool,
            &|_, _| {},
            &persistent,
            &mut planner,
            &mut committer,
            [job(1, contract).tx, job(2, contract).tx],
        )
        .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(resolutions.len(), 1);
        assert!(resolutions[0].committable.is_empty());
        assert_eq!(
            resolutions[0]
                .conflicts
                .iter()
                .map(|(left, right)| (left.global, right.global))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(0, 1)])
        );
        let count = crate::planner::history::Field::Count.slot(0);
        assert_eq!(
            resolutions[0].history[&contract].storage[&count].present_value(),
            U256::from(1)
        );
        assert_eq!(planner.db.storage(contract, count).unwrap(), U256::ZERO);
        assert_eq!(
            persistent.storage_ref(contract, U256::ZERO).unwrap(),
            U256::ZERO
        );
    }
}
