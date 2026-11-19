use super::*;
use crate::execution::evm::state::StateOverlay;
use crate::execution::{ArcologyBlockExecutorFactory, ArcologyTx, CallRouter, StateDb};
use crate::planner::resolver::{
    CommitDecision, CommitVerdict, ConflictDetector, accepted_ids, rejected_ids,
};
use crate::planner::scheduler::{Generation, JobSequence};
use alloy_evm::{
    Evm, EvmEnv, FromRecoveredTx, RecoveredTx,
    block::BlockExecutor,
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
        .map(|raw| &raw.verdict)
        .flat_map(|verdict| {
            verdict
                .conflicts
                .iter()
                .flat_map(|(peers, _)| peers)
                .map(move |peer| (verdict.id.min(*peer), verdict.id.max(*peer)))
        })
        .collect()
}

struct Recovered {
    tx: OpTxEnvelope,
    signer: Address,
}

#[test]
fn job_returns_raw_state_and_commits_to_database() {
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
            StateDb::with_fallback(
                StateOverlay::new(EvmState::default(), StateOverlay::copy_balance_nonce),
                Some(&fallback),
            ),
            env,
            crate::execution::Recorder::new(job.id.global),
        );
        let mut executor = factory.create_executor(evm, OpBlockExecutionCtx::default());
        let mut raw = job.run(&mut executor);
        let committed = executor.evm().db().inner();
        if gas_limit == 100_000 {
            raw.verdict.decision = CommitDecision::Accepted;
            assert!(raw.receipt.is_some());
            let executed = raw.result_and_state().as_ref().unwrap();
            assert!(executed.result.is_success());
            assert!(executed.state[&caller].is_loaded_as_not_existing());
            assert!(!committed[&caller].is_loaded_as_not_existing());
            assert_eq!(committed[&caller].info.nonce, 1);
        } else {
            assert!(raw.result_and_state().is_err());
            assert!(raw.receipt.is_none());
            assert!(raw.fee.is_zero());
            assert!(raw.fee_recipients.is_empty());
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
            job.meta.is_deferred = is_deferred;
            job
        })
        .collect();
    let (mut outcomes, _) = JobSequence {
        jobs,
        nonce_offsets: Vec::new(),
    }
    .run(&factory, &fallback, env, OpBlockExecutionCtx::default())
    .unwrap();
    for (outcome, expected) in outcomes.iter_mut().zip([true, false]) {
        let executed = outcome.result_and_state().as_ref().unwrap();
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
fn job_callee_contains_full_address_and_padded_selector() {
    use alloy_evm::revm::context::TxEnv;

    let contract = Address::with_last_byte(42);
    let tx = TxEnv {
        kind: TxKind::Call(contract),
        data: vec![0x12, 0x34].into(),
        ..Default::default()
    };
    let mut callee = Job::new(ID::new(0, 0), (tx, ())).meta.id.unwrap();
    assert_eq!(callee.account, contract);
    assert_eq!(callee.selector, [0x12, 0x34, 0, 0]);
    assert_eq!(callee.series, 0);
    callee.series = 7;
    assert_eq!(callee.series, 7);
}

#[test]
fn job_detects_deposit_transaction_type() {
    let deposit = job(0, Address::repeat_byte(0x99));
    assert!(deposit.is_l1_deposit);
}

#[test]
fn parallel_storage_reads_do_not_conflict() {
    use alloy_evm::revm::state::EvmStorageSlot;

    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let mut env = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::REGOLITH;
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let contract = Address::repeat_byte(0x99);
    // SLOAD(0), POP, STOP. Neither transaction writes storage.
    let code = Bytecode::new_raw(Bytes::from_static(&[0x60, 0, 0x54, 0x50, 0]));

    for value in [0, 7] {
        let mut account = Account::from(AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code.clone()),
            ..Default::default()
        });
        account.storage.insert(
            U256::ZERO,
            EvmStorageSlot::new(U256::from(value), Default::default()),
        );
        let fallback = StateDb::new(EvmState::from_iter([(contract, account)]));
        let generation = Generation::new(
            [0, 1]
                .into_iter()
                .map(|id| JobSequence::new(vec![job(id, contract)]))
                .collect(),
        );
        let (mut outcomes, committed) = generation
            .run(
                &factory,
                &config,
                &pool,
                &mut ConflictDetector::new(),
                &fallback,
            )
            .unwrap();
        assert!(
            outcomes
                .iter_mut()
                .all(|raw| { raw.result_and_state().as_ref().unwrap().result.is_success() })
        );
        assert_eq!(
            committed[&contract].storage[&U256::ZERO].present_value(),
            U256::from(value)
        );
        assert!(
            pairs(&outcomes).is_empty(),
            "read-only slot containing {value} produced conflicts: {:?}",
            pairs(&outcomes)
        );
        assert_eq!(
            accepted_ids(outcomes.iter().map(|raw| &raw.verdict)).len(),
            2
        );
    }
}

#[test]
fn single_sequence_gets_accepted_verdicts_from_checker() {
    use crate::planner::resolver::Checker;

    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    );
    let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::BEDROCK;
    env.block_env.gas_limit = 1_000_000;
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let generation = Generation {
        sequences: vec![JobSequence {
            jobs: vec![job(1, Address::repeat_byte(0x99))],
            nonce_offsets: Vec::new(),
        }],
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let mut checker = ConflictDetector::new();
    let (outcomes, _) = generation
        .run(&factory, &config, &pool, &mut checker, &StateDb::default())
        .unwrap();
    assert_eq!(outcomes[0].verdict.decision, CommitDecision::Accepted);
    assert!(checker.check().is_empty());
}

#[test]
fn sequence_nonce_offsets_are_local_and_checked() {
    use crate::execution::EvmStateCommitter;
    use crate::store::traits::Committer;

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
            let (mut outcomes, committed) = result.unwrap();
            let executed = outcomes[0].result_and_state().as_ref().unwrap();
            assert!(executed.result.is_success());
            assert_eq!(executed.state[&caller].info.nonce, expected);
            assert_eq!(committed[&caller].info.nonce, expected);
            assert_eq!(committed[&caller].original_info().nonce, nonce + offset);
            let mut generation = EvmStateCommitter::new_parallel();
            generation.buffer(ID::new(0, 0), committed);
            let committed = generation.commit(&fallback).unwrap();
            assert_eq!(committed[&caller].info.nonce, nonce + 1);
            assert_eq!(committed[&caller].original_info().nonce, nonce);
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
        let (mut outcomes, committed_state) = generation
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
        for result in &mut outcomes {
            let accepted = result.verdict.decision == CommitDecision::Accepted;
            assert_eq!(
                result
                    .result_and_state()
                    .as_ref()
                    .unwrap()
                    .result
                    .is_success(),
                accepted
            );
            assert_eq!(result.receipt.as_ref().unwrap().status(), accepted);
        }
        assert_eq!(
            accepted_ids(outcomes.iter().map(|raw| &raw.verdict))
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
        assert_eq!(
            committed_state[&Address::with_last_byte(0x12)].info.nonce,
            1
        );
        let rejected = &outcomes
            .iter()
            .find(|raw| raw.id.global == 2)
            .unwrap()
            .verdict;
        assert_eq!(rejected.decision, CommitDecision::Rejected);
        let CommitVerdict { conflicts, .. } = rejected;
        assert!(
            conflicts
                .iter()
                .any(|(peers, _)| peers.contains(&job(1, contract).id))
        );
        assert!(
            conflicts
                .iter()
                .any(|(_, reason)| matches!(reason, crate::planner::resolver::Reason::AccessConflict(key, _) if *key == Key::Storage(contract, U256::ZERO)))
        );

        let generation = Generation {
            sequences: vec![JobSequence {
                jobs: vec![job(3, contract)],
                nonce_offsets: Vec::new(),
            }],
        };

        let fallback = StateDb::with_fallback(
            StateOverlay::new(committed_state, StateOverlay::copy_balance_nonce),
            Some(&persistent),
        );
        let (mut outcomes, committed_state) = generation
            .run(&factory, &config, &pool, &mut checker, &fallback)
            .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(
            accepted_ids(outcomes.iter().map(|raw| &raw.verdict))
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([outcomes[0].id])
        );
        assert!(rejected_ids(outcomes.iter().map(|raw| &raw.verdict)).is_empty());
        assert!(pairs(&outcomes).is_empty());
        assert_eq!(
            outcomes[0].result_and_state().as_ref().unwrap().state[&contract].storage[&U256::ZERO]
                .present_value(),
            U256::from(2)
        );
        assert_eq!(
            committed_state[&contract].storage[&U256::ZERO].present_value(),
            U256::from(2)
        );

        let mut planner = Planner::new(StateDb::new(state.clone()), Default::default());
        let (outcomes, committed) = ArcologyParaBlockExecutor::execute_block(
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
            U256::from(2)
        );
        assert_eq!(
            accepted_ids(outcomes.iter().map(|raw| &raw.verdict))
                .into_iter()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([outcomes[0].id, outcomes[1].id])
        );
        assert!(pairs(&outcomes).is_empty());
        let selector = job(1, contract).meta.id.unwrap().selector;
        let count = crate::planner::meta::MetaFieldId::ConflictCount.slot(contract, selector, 0);
        assert!(!committed[&contract].storage.contains_key(&count));
        assert_eq!(planner.db.storage(contract, count).unwrap(), U256::ZERO);
        assert_eq!(
            persistent.storage_ref(contract, U256::ZERO).unwrap(),
            U256::ZERO
        );
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
    let (outcomes, committed_state) = generation
        .run(&factory, &config, &pool, &mut checker, &fallback)
        .unwrap();
    assert!(rejected_ids(outcomes.iter().map(|raw| &raw.verdict)).is_empty());
    assert_eq!(
        accepted_ids(outcomes.iter().map(|raw| &raw.verdict))
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
    let (mut outcomes, _) = ArcologyParaBlockExecutor::execute_block(
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
            .result_and_state()
            .as_ref()
            .unwrap()
            .result
            .is_success()
    );
    assert_eq!(
        accepted_ids(outcomes.iter().map(|raw| &raw.verdict)),
        vec![id]
    );
}

#[test]
fn deposits_run_in_order_and_preserve_storage_and_sender_nonces() {
    use crate::planner::meta::MetaFieldId;
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
    let selector = job(1, contract).meta.id.unwrap().selector;
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0,
    ]));
    let mut account = Account::from(AccountInfo {
        code_hash: code.hash_slow(),
        code: Some(code),
        ..Default::default()
    });
    // Deposits remain in one ordered sequence even when the callee is deferrable.
    account.storage.insert(
        MetaFieldId::DeferredGas.slot(contract, selector, 0),
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
        assert_eq!(block.generations.len(), 1);
        let cumulative = std::cell::RefCell::new(Vec::new());
        let (mut outcomes, committed) = block
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
        for (index, outcome) in outcomes.iter_mut().enumerate() {
            assert_eq!(outcome.id.block, 42);
            assert_eq!(outcome.id.generation, 0);
            assert_eq!(outcome.id.job, index as u64);
            let executed = outcome.result_and_state().as_ref().unwrap();
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
            assert_eq!(outcome.verdict.id, outcome.id);
            assert_eq!(outcome.verdict.decision, CommitDecision::Accepted);
        }
        assert_eq!(
            fallback.storage_ref(contract, U256::ZERO).unwrap(),
            U256::ZERO
        );
        assert!(fallback.basic_ref(caller).unwrap().is_none());
    }
}

#[test]
fn sequence_keeps_api_transitions_for_rejected_suffixes() {
    use crate::crdt::state::{Status, Tracked};
    use crate::execution::{
        ApiAddress, IU256, ScalarAdapter, U256Handler, output::ExecutionOutput,
    };
    use crate::planner::resolver::Checker;
    use alloy_evm::revm::state::EvmStorageSlot;
    use alloy_sol_types::SolCall;

    struct Verdicts(Vec<CommitVerdict<Key>>);
    impl Checker<Key> for Verdicts {
        fn import(&mut self, _: Vec<ExecutionOutput<Key, Tracked<Status, Status>>>) {}

        fn check(&mut self) -> Vec<CommitVerdict<Key>> {
            std::mem::take(&mut self.0)
        }
    }

    let api = ApiAddress::U256.address();
    let mut router = CallRouter::new();
    router.register(api, U256Handler).unwrap();
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(router).unwrap(),
    );
    let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::REGOLITH;
    env.block_env.timestamp = U256::from(1_700_000_000u64);
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let caller = Address::from(job(1, api).sender);
    let handle = Address::with_last_byte(7);
    let key = crate::execution::ScalarKeyCodec::encode(
        crate::namespace::Namespace::U256,
        crate::execution::Property::Value as u8,
        handle,
    );
    let slots = ScalarAdapter::new(key);
    let mut account = Account::from(AccountInfo::default().with_nonce(7));
    account.storage.insert(
        slots.max,
        EvmStorageSlot::new(U256::from(100), Default::default()),
    );
    let fallback = StateDb::new(EvmState::from_iter([(caller, account)]));

    for reject_last in [false, true] {
        let jobs = [2, 3, 5]
            .into_iter()
            .enumerate()
            .map(|(index, delta)| {
                let tx = OpTxEnvelope::from(TxDeposit {
                    from: caller,
                    to: TxKind::Call(api),
                    input: IU256::addCall {
                        key: key.into(),
                        delta: U256::from(delta),
                    }
                    .abi_encode()
                    .into(),
                    gas_limit: 100_000,
                    ..Default::default()
                });
                Job::new(
                    ID {
                        job: index as u64,
                        ..ID::new(0, index as u64)
                    },
                    (
                        ArcologyTx::from_recovered_tx(&tx, caller),
                        Recovered { tx, signer: caller },
                    ),
                )
            })
            .collect::<Vec<_>>();
        let mut checker = Verdicts(
            jobs.iter()
                .enumerate()
                .map(|(index, job)| CommitVerdict {
                    id: job.id,
                    decision: if reject_last && index == 2 {
                        CommitDecision::Rejected
                    } else {
                        CommitDecision::Accepted
                    },
                    conflicts: Vec::new(),
                })
                .collect(),
        );
        let (mut outcomes, committed) = Generation {
            sequences: vec![JobSequence {
                jobs,
                nonce_offsets: vec![crate::planner::scheduler::sender::SenderNonceOffset {
                    sender: caller.into_array(),
                    offset: 2,
                }],
            }],
        }
        .run(&factory, &config, &pool, &mut checker, &fallback)
        .unwrap();
        let account = &committed[&caller];
        assert_eq!(account.original_info().nonce, 7);
        assert_eq!(account.info.nonce, 10);
        assert_eq!(
            account.storage[&slots.pos_delta].original_value(),
            U256::ZERO
        );
        assert_eq!(
            account.storage[&slots.pos_delta].present_value(),
            U256::from(if reject_last { 5 } else { 10 })
        );
        for (index, raw) in outcomes.iter_mut().take(2).enumerate() {
            let executed = raw.result_and_state().as_ref().unwrap();
            assert!(executed.result.is_success());
            let state = &executed.state;
            assert_eq!(state[&caller].original_info().nonce, 9 + index as u64);
            assert_eq!(
                state[&caller].storage[&slots.pos_delta].original_value(),
                U256::from([0, 2][index])
            );
            assert_eq!(
                state[&caller].storage[&slots.pos_delta].present_value(),
                U256::from([2, 5][index])
            );
        }
        assert_eq!(
            fallback.storage_ref(caller, slots.pos_delta).unwrap(),
            U256::ZERO
        );
        assert_eq!(fallback.basic_ref(caller).unwrap().unwrap().nonce, 7);
    }
}

#[test]
fn execution_errors_are_reported_without_committing_state() {
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
    let result = generation.run(&factory, &config, &pool, &mut checker, &StateDb::default());
    assert!(matches!(result, Err(error) if error.to_string().contains(
        "transaction gas limit 100001 is more than blocks available gas 100000"
    )));
}
