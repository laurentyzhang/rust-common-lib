use super::*;
use crate::execution::{ArcologyBlockExecutorFactory, ArcologyTx, CallRouter, StateDb};
use crate::planner::resolver::{
    CommitVerdict, ConflictDetector, Reason, accepted_ids, rejected_ids,
};
use crate::planner::scheduler::{Generation, JobSequence};
use alloy_evm::{
    EvmEnv, FromRecoveredTx, RecoveredTx,
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

fn pairs<R, H>(raws: &[JobResult<R, H>]) -> BTreeSet<(ID, ID)> {
    raws.iter()
        .filter_map(|raw| raw.verdict.as_ref())
        .flat_map(|verdict| {
            verdict
                .peers
                .iter()
                .map(move |peer| (verdict.id.min(*peer), verdict.id.max(*peer)))
        })
        .collect()
}

struct Recovered {
    tx: OpTxEnvelope,
    signer: Address,
}

#[test]
fn job_returns_raw_and_committed_states() {
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let fallback = StateDb::default();
    for gas_limit in [100_000, 99_999] {
        let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
        env.cfg_env.spec = OpSpecId::BEDROCK;
        env.block_env.gas_limit = gas_limit;
        let job = job(1, Address::repeat_byte(0x99));
        let caller = Address::from(job.sender);
        let evm = factory.evm_factory().create_evm_with_inspector(
            StateDb::with_fallback(EvmState::default(), Some(&fallback)),
            env,
            crate::execution::ReadRecorder::new(job.id.global),
        );
        let mut executor = factory.create_executor(evm, OpBlockExecutionCtx::default());
        let StateOutcome { raw, committed } = job.run(&mut executor);
        if gas_limit == 100_000 {
            let executed = raw.result_and_state.unwrap();
            assert!(executed.result.is_success());
            assert!(raw.receipt.is_some());
            assert!(executed.state[&caller].is_loaded_as_not_existing());
            assert!(!committed[&caller].is_loaded_as_not_existing());
            assert_eq!(committed[&caller].info.nonce, 1);
        } else {
            assert!(raw.result_and_state.is_err());
            assert!(raw.receipt.is_none());
            assert!(committed.is_empty());
        }
        assert!(fallback.basic_ref(caller).unwrap().is_none());
    }
}

#[test]
fn scheduled_jobs_expose_their_own_deferred_phase() {
    use crate::execution::{ApiAddress, IRuntime, RuntimeHandler};
    use alloy_sol_types::{SolCall, SolValue};

    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), RuntimeHandler)
        .unwrap();
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(router).unwrap(),
    );
    let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::REGOLITH;
    env.block_env.timestamp = U256::from(1_700_000_000u64);
    env.block_env.gas_limit = 200_000;
    let contract = Address::repeat_byte(0x99);
    let mut code = vec![0x63];
    code.extend_from_slice(&IRuntime::isDeferredCall::SELECTOR);
    code.extend_from_slice(&[
        0x60, 0xe0, 0x1b, 0x60, 0, 0x52, 0x60, 32, 0x60, 0, 0x60, 4, 0x60, 0, 0x60, 0xa0, 0x61,
        0xff, 0xff, 0xfa, 0x50, 0x60, 32, 0x60, 0, 0xf3,
    ]);
    let code = Bytecode::new_raw(code.into());
    let fallback = StateDb::new(EvmState::from_iter([(
        contract,
        Account::from(AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        }),
    )]));
    let jobs = [(1, true), (2, false)]
        .into_iter()
        .map(|(id, is_deferred)| {
            let mut job = job(id, contract);
            job.is_deferred = is_deferred;
            job
        })
        .collect();
    let StateOutcome { raw: outcomes, .. } = JobSequence {
        jobs,
        nonce_offsets: Vec::new(),
    }
    .run(&factory, &fallback, env, OpBlockExecutionCtx::default())
    .unwrap();
    for (outcome, expected) in outcomes.iter().zip([true, false]) {
        let executed = outcome.result_and_state.as_ref().unwrap();
        assert!(executed.result.is_success());
        assert_eq!(
            bool::abi_decode_validate(executed.result.output().unwrap()).unwrap(),
            expected
        );
    }
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
            ..ID::new(0, id)
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
        ID::new(0, 2),
        (
            ArcologyTx::from_recovered_tx(&tx, caller),
            Recovered { tx, signer: caller },
        ),
    );
    let StateOutcome {
        raw: mut outcomes,
        committed,
    } = JobSequence {
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
    assert!(outcomes[1].to_records(outcomes[1].id).is_empty());
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
    assert!(!committed.contains_key(&caller));
    assert_eq!(
        committed[&contract].storage[&U256::ZERO].present_value(),
        U256::from(2)
    );

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
            let StateOutcome {
                raw: outcomes,
                committed,
            } = result.unwrap();
            let executed = outcomes[0].result_and_state.as_ref().unwrap();
            assert!(executed.result.is_success());
            assert_eq!(executed.state[&caller].info.nonce, expected);
            assert_eq!(committed[&caller].info.nonce, expected);
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
        let mut checker = ConflictDetector::new();
        let order = if workers == 1 { [2, 1] } else { [1, 2] };
        let generation = Generation {
            sequences: order
                .into_iter()
                .map(|id| JobSequence {
                    jobs: vec![job(id, contract)],
                    nonce_offsets: Vec::new(),
                })
                .collect(),
        };
        let StateOutcome {
            raw: outcomes,
            committed: committed_state,
        } = generation
            .run(&factory, &config, &pool, &mut checker, &persistent)
            .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(
            outcomes
                .iter()
                .map(|outcome| outcome.id.global)
                .collect::<Vec<_>>(),
            order
        );
        assert!(outcomes.iter().all(|result| {
            result
                .result_and_state
                .as_ref()
                .unwrap()
                .result
                .is_success()
        }));
        assert_eq!(
            accepted_ids(outcomes.iter().filter_map(|raw| raw.verdict.as_ref()))
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([job(1, contract).id])
        );
        assert_eq!(
            pairs(&outcomes)
                .iter()
                .map(|(left, right)| (left.global, right.global))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(1, 2)])
        );
        assert_eq!(
            committed_state[&contract].storage[&U256::ZERO].present_value(),
            U256::from(1)
        );
        assert!(!committed_state.contains_key(&Address::with_last_byte(0x12)));
        let rejected = outcomes
            .iter()
            .find(|raw| raw.id.global == 2)
            .unwrap()
            .verdict
            .as_ref()
            .unwrap();
        assert!(!rejected.accepted);
        let CommitVerdict { peers, reasons, .. } = rejected;
        assert_eq!(peers, &vec![job(1, contract).id]);
        assert!(
            reasons
                .iter()
                .any(|(key, _)| *key == Some(Key::Storage(contract, U256::ZERO)))
        );

        let generation = Generation {
            sequences: vec![JobSequence {
                jobs: vec![job(3, contract)],
                nonce_offsets: Vec::new(),
            }],
        };

        let fallback = StateDb::with_fallback(committed_state, Some(&persistent));
        let StateOutcome {
            raw: outcomes,
            committed: committed_state,
        } = generation
            .run(&factory, &config, &pool, &mut checker, &fallback)
            .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            accepted_ids(outcomes.iter().filter_map(|raw| raw.verdict.as_ref()))
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([outcomes[0].id])
        );
        assert!(rejected_ids(outcomes.iter().filter_map(|raw| raw.verdict.as_ref())).is_empty());
        assert!(pairs(&outcomes).is_empty());
        assert_eq!(
            outcomes[0].result_and_state.as_ref().unwrap().state[&contract].storage[&U256::ZERO]
                .present_value(),
            U256::from(2)
        );
        assert_eq!(
            committed_state[&contract].storage[&U256::ZERO].present_value(),
            U256::from(2)
        );

        let mut planner = Planner::new(StateDb::new(state.clone()), Default::default());
        let StateOutcome {
            raw: outcomes,
            committed,
        } = ArcologyParaBlockExecutor::execute_block(
            &factory,
            &config,
            &pool,
            &|_, _| {},
            &persistent,
            &mut planner,
            [job(1, contract).tx, job(2, contract).tx],
        )
        .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(
            committed[&contract].storage[&U256::ZERO].present_value(),
            U256::from(1)
        );
        assert_eq!(
            accepted_ids(outcomes.iter().filter_map(|raw| raw.verdict.as_ref()))
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([outcomes[0].id])
        );
        assert_eq!(
            pairs(&outcomes)
                .iter()
                .map(|(left, right)| (left.global, right.global))
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([(0, 1)])
        );
        let callee = job(1, contract).callee.unwrap();
        let count =
            crate::planner::meta::property::Property::ConflictCount.slot(contract, callee, 0);
        assert_eq!(
            committed[&contract].storage[&count].present_value(),
            U256::from(1)
        );
        assert_eq!(planner.db.storage(contract, count).unwrap(), U256::ZERO);
        assert_eq!(
            persistent.storage_ref(contract, U256::ZERO).unwrap(),
            U256::ZERO
        );
    }
}

#[test]
fn generation_commits_finalized_sequence_prefixes() {
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
    let contracts = [
        Address::repeat_byte(0x77),
        Address::repeat_byte(0x88),
        Address::repeat_byte(0x99),
    ];
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0,
    ]));
    let fallback = StateDb::new(EvmState::from_iter(contracts.map(|contract| {
        (
            contract,
            Account::from(AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code.clone()),
                ..Default::default()
            }),
        )
    })));
    for workers in [1, 2] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        let sequences = vec![
            JobSequence {
                jobs: contracts
                    .into_iter()
                    .enumerate()
                    .map(|(index, contract)| job(index as u64, contract))
                    .collect(),
                nonce_offsets: Vec::new(),
            },
            JobSequence {
                jobs: (3..8).map(|id| job(id, contracts[1])).collect(),
                nonce_offsets: Vec::new(),
            },
        ];
        let mut generation = Generation { sequences };
        generation.assign_ids(0);
        let mut checker = ConflictDetector::new();
        let outcome = generation
            .run(&factory, &config, &pool, &mut checker, &fallback)
            .unwrap();

        assert_eq!(
            rejected_ids(
                &outcome
                    .raw
                    .iter()
                    .filter_map(|raw| raw.verdict.clone())
                    .collect::<Vec<_>>()
            )
            .iter()
            .map(|id| id.global)
            .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(outcome.raw.len(), 8);
        assert!(
            outcome.raw[2]
                .verdict
                .as_ref()
                .unwrap()
                .reasons
                .contains(&(None, Reason::Dependency(outcome.raw[1].id)))
        );
        assert_eq!(
            outcome.committed[&contracts[0]].storage[&U256::ZERO].present_value(),
            U256::from(1)
        );
        assert_eq!(
            outcome.committed[&contracts[1]].storage[&U256::ZERO].present_value(),
            U256::from(5)
        );
        assert!(!outcome.committed.contains_key(&contracts[2]));
        for id in [1, 2] {
            assert!(
                !outcome
                    .committed
                    .contains_key(&Address::from(job(id, contracts[1]).sender))
            );
        }
        for contract in contracts {
            assert_eq!(
                fallback.storage_ref(contract, U256::ZERO).unwrap(),
                U256::ZERO
            );
        }
    }
}

#[test]
fn commits_all_conflict_free_parallel_jobs() {
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
    let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 1, 0x60, 0, 0x55, 0]));
    let contracts = [Address::repeat_byte(0x88), Address::repeat_byte(0x99)];
    let state = EvmState::from_iter(contracts.map(|contract| {
        (
            contract,
            Account::from(AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code.clone()),
                ..Default::default()
            }),
        )
    }));
    let fallback = StateDb::new(state);
    let mut checker = ConflictDetector::new();
    let generation = Generation {
        sequences: contracts
            .into_iter()
            .enumerate()
            .map(|(index, contract)| JobSequence {
                jobs: vec![job(index as u64 + 1, contract)],
                nonce_offsets: Vec::new(),
            })
            .collect(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let StateOutcome {
        raw: outcomes,
        committed: committed_state,
    } = generation
        .run(&factory, &config, &pool, &mut checker, &fallback)
        .unwrap();
    assert!(rejected_ids(outcomes.iter().filter_map(|raw| raw.verdict.as_ref())).is_empty());
    assert_eq!(
        accepted_ids(outcomes.iter().filter_map(|raw| raw.verdict.as_ref()))
            .into_iter()
            .collect::<BTreeSet<_>>(),
        outcomes.iter().map(|outcome| outcome.id).collect()
    );
    for contract in contracts {
        assert_eq!(
            committed_state[&contract].storage[&U256::ZERO].present_value(),
            U256::from(1)
        );
        assert_eq!(
            fallback.storage_ref(contract, U256::ZERO).unwrap(),
            U256::ZERO
        );
    }
}

#[test]
fn block_execution_carries_the_environment_block_number_into_ids() {
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::BEDROCK;
    env.block_env.number = U256::from(42);
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let fallback = StateDb::default();
    let mut planner = Planner::new(StateDb::default(), Default::default());
    let StateOutcome { raw: outcomes, .. } = ArcologyParaBlockExecutor::execute_block(
        &factory,
        &config,
        &pool,
        &|_, _| {},
        &fallback,
        &mut planner,
        [job(1, Address::repeat_byte(0x99)).tx],
    )
    .unwrap();
    let id = ID::new(42, 0);
    assert_eq!(outcomes[0].id, id);
    assert!(
        outcomes[0]
            .result_and_state
            .as_ref()
            .unwrap()
            .result
            .is_success()
    );
    assert_eq!(
        accepted_ids(outcomes.iter().filter_map(|raw| raw.verdict.as_ref())),
        vec![id]
    );
}

#[test]
fn later_generations_read_committed_storage_and_sender_nonces() {
    use crate::planner::meta::property::Property;
    use alloy_evm::revm::state::EvmStorageSlot;

    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::BEDROCK;
    env.block_env.number = U256::from(42);
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let contract = Address::repeat_byte(0x99);
    let callee = job(1, contract).callee.unwrap();
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0,
    ]));
    let mut account = Account::from(AccountInfo {
        code_hash: code.hash_slow(),
        code: Some(code),
        ..Default::default()
    });
    // A repeated deferrable call runs in the following generation.
    account.storage.insert(
        Property::DeferredGas.slot(contract, callee, 0),
        EvmStorageSlot::new(U256::from(1), Default::default()),
    );
    let state = EvmState::from_iter([(contract, account)]);
    let fallback = StateDb::new(state.clone());
    for workers in [1, 2] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        let mut planner = Planner::new(StateDb::new(state.clone()), Default::default());
        let caller = Address::from(job(1, contract).sender);
        let mut jobs = [job(1, contract), job(1, contract)];
        for (index, job) in jobs.iter_mut().enumerate() {
            job.id = ID::new(42, index as u64);
        }
        let block = planner.plan(jobs).unwrap();
        assert_eq!(block.generations.len(), 2);
        let cumulative = std::cell::RefCell::new(Vec::new());
        let StateOutcome {
            raw: outcomes,
            committed,
        } = block
            .run(
                &factory,
                &config,
                &pool,
                &|_, gas| cumulative.borrow_mut().push(gas),
                &fallback,
                &mut planner,
            )
            .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(
            committed[&contract].storage[&U256::ZERO].present_value(),
            U256::from(2)
        );
        assert_eq!(committed[&caller].info.nonce, 2);
        let mut expected_gas = 0;
        for (index, outcome) in outcomes.iter().enumerate() {
            assert_eq!(outcome.id.block, 42);
            assert_eq!(outcome.id.generation, index as u64);
            let executed = outcome.result_and_state.as_ref().unwrap();
            expected_gas += executed.result.tx_gas_used();
            assert_eq!(cumulative.borrow()[index], expected_gas);
            assert!(executed.result.is_success());
            assert_eq!(
                executed.state[&contract].storage[&U256::ZERO].original_value(),
                U256::from(index)
            );
            assert_eq!(
                executed.state[&contract].storage[&U256::ZERO].present_value(),
                U256::from(index + 1)
            );
            assert_eq!(executed.state[&caller].info.nonce, index as u64 + 1);
            assert_eq!(outcome.verdict.as_ref().unwrap().id, outcome.id);
            assert!(outcome.verdict.as_ref().unwrap().accepted);
        }
        assert_eq!(
            fallback.storage_ref(contract, U256::ZERO).unwrap(),
            U256::ZERO
        );
        assert!(fallback.basic_ref(caller).unwrap().is_none());
    }
}

#[test]
fn execution_errors_have_no_commit_verdict_or_committed_state() {
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::BEDROCK;
    env.block_env.gas_limit = 100_000;
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let caller = Address::repeat_byte(0x22);
    let tx = OpTxEnvelope::from(TxDeposit {
        from: caller,
        to: TxKind::Call(Address::repeat_byte(0x99)),
        gas_limit: 100_001,
        ..Default::default()
    });
    let job = Job::new(
        ID::new(42, 7),
        (
            ArcologyTx::from_recovered_tx(&tx, caller),
            Recovered { tx, signer: caller },
        ),
    );
    let generation = Generation {
        sequences: vec![JobSequence {
            jobs: vec![job],
            nonce_offsets: Vec::new(),
        }],
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut checker = ConflictDetector::new();
    let StateOutcome {
        raw: outcomes,
        committed: committed_state,
    } = generation
        .run(&factory, &config, &pool, &mut checker, &StateDb::default())
        .unwrap();
    assert!(outcomes[0].result_and_state.is_err());
    assert!(outcomes[0].verdict.is_none());
    assert!(outcomes[0].receipt.is_none());
    assert!(outcomes.iter().all(|raw| raw.verdict.is_none()));
    assert!(committed_state.is_empty());
}
