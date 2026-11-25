use crate::execution::evm::state::StateOverlay;
use super::generation_tests::block_state;
use crate::execution::{
    ArcologyBlockExecutorFactory, ArcologyEvmFactory, ArcologyTx, CallRouter, JobResult,
    ParaBlockConfig, Recorder, StateDb,
    workload::{Generation, ID, Job, JobSequence},
};
use crate::planner::resolver::{CommitDecision, CommitVerdict, ConflictDetector};
use alloy_evm::{
    EvmEnv, EvmFactory, FromRecoveredTx, RecoveredTx,
    block::BlockExecutorFactory,
    revm::{
        DatabaseRef,
        state::{Account, AccountInfo, Bytecode, EvmState},
    },
};
use alloy_op_evm::{OpBlockExecutionCtx, block::OpAlloyReceiptBuilder};
use alloy_op_hardforks::OpChainHardforks;
use alloy_primitives::{Address, Bytes, Signature, TxKind, U256};
use op_alloy::consensus::{OpTxEnvelope, OpTypedTransaction};
use op_revm::{OpSpecId, constants::*};

const FUNDS: u64 = 10_000_000;

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

fn job(
    id: u64,
    caller: Address,
    to: TxKind,
    value: u64,
    input: Bytes,
) -> Job<(ArcologyTx, Recovered)> {
    let mut typed = OpTypedTransaction::Legacy(Default::default());
    let OpTypedTransaction::Legacy(tx) = &mut typed else {
        unreachable!()
    };
    tx.to = to;
    tx.value = U256::from(value);
    tx.input = input;
    tx.gas_limit = 100_000;
    tx.gas_price = 10;
    let tx = OpTxEnvelope::new_unhashed(typed, Signature::new(U256::from(1), U256::from(1), false));
    Job::new(
        ID::new(0, id),
        (
            ArcologyTx::from_recovered_tx(&tx, caller),
            Recovered { tx, signer: caller },
        ),
    )
}

#[test]
fn job_detects_regular_transaction_type() {
    let regular = job(
        0,
        Address::repeat_byte(0x11),
        TxKind::Create,
        0,
        Bytes::new(),
    );
    assert!(!regular.is_l1_deposit);
}

fn factory() -> ArcologyBlockExecutorFactory<OpAlloyReceiptBuilder, OpChainHardforks> {
    ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
    )
}

fn env(beneficiary: Address) -> EvmEnv<OpSpecId> {
    let mut env: EvmEnv<OpSpecId> = EvmEnv::default();
    env.cfg_env.spec = OpSpecId::REGOLITH;
    env.block_env.timestamp = U256::from(1_700_000_000u64);
    env.block_env.beneficiary = beneficiary;
    env.block_env.basefee = 3;
    env.block_env.gas_limit = 1_000_000;
    env
}

fn account(balance: u64, code: Bytes) -> Account {
    let code = Bytecode::new_raw(code);
    Account::from(AccountInfo {
        balance: U256::from(balance),
        code_hash: code.hash_slow(),
        code: Some(code),
        ..Default::default()
    })
}

fn reject<R: crate::execution::ReceiptStatus, H: Clone>(
    mut outcome: JobResult<R, H>,
    fallback: &impl DatabaseRef,
) -> EvmState {
    outcome.verdict = CommitVerdict {
        id: outcome.id,
        decision: CommitDecision::Rejected,
        conflicts: Vec::new(),
    };
    JobSequence::<()>::rebuild_sequence_state(&mut [outcome], fallback).unwrap()
}

#[test]
fn parallel_conflicts_keep_every_fee_and_nonce() {
    let factory = factory();
    let contract = Address::repeat_byte(0x99);
    let beneficiary = Address::repeat_byte(0x88);
    let callers = [
        Address::repeat_byte(0x11),
        Address::repeat_byte(0x22),
        Address::repeat_byte(0x33),
    ];
    let mut state =
        EvmState::from_iter(callers.map(|caller| (caller, account(FUNDS, Bytes::new()))));
    state.insert(
        contract,
        account(
            0,
            Bytes::from_static(&[0x60, 0, 0x54, 0x60, 1, 0x01, 0x60, 0, 0x55, 0]),
        ),
    );
    let fallback = StateDb::new(state);
    let config = ParaBlockConfig {
        env: env(beneficiary),
        ctx: OpBlockExecutionCtx::default(),
    };
    for workers in [1, 3] {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(workers)
            .build()
            .unwrap();
        let mut generation = Generation {
            sequences: callers
                .into_iter()
                .enumerate()
                .map(|(id, caller)| JobSequence {
                    jobs: vec![job(
                        id as u64,
                        caller,
                        TxKind::Call(contract),
                        123,
                        Bytes::new(),
                    )],
                    nonce_offsets: Vec::new(),
                })
                .collect(),
        };
        generation.assign_ids(0);
        let (outcomes, committed) = generation
            .run(
                &factory,
                &config,
                &pool,
                &mut ConflictDetector::new(),
                block_state(&fallback),
            )
            .map(|(raws, state)| (raws, state.into_inner()))
            .unwrap();
        let mut gas = 0;
        let mut accepted = 0;
        for mut raw in outcomes {
            let used = raw
                .result_and_state()
                .as_ref()
                .unwrap()
                .result
                .tx_gas_used();
            gas += used;
            let is_accepted = raw.verdict.decision == CommitDecision::Accepted;
            accepted += u64::from(is_accepted);
            let caller = callers[raw.id.global as usize];
            assert_eq!(committed[&caller].info.nonce, 1);
            assert_eq!(
                committed[&caller].info.balance,
                U256::from(FUNDS - used * 10 - if is_accepted { 123 } else { 0 })
            );
            assert!(raw.receipt.is_some());
        }
        assert_eq!(accepted, 1);
        assert_eq!(committed[&beneficiary].info.balance, U256::from(gas * 7));
        assert_eq!(
            committed[&BASE_FEE_RECIPIENT].info.balance,
            U256::from(gas * 3)
        );
        assert_eq!(committed[&contract].info.balance, U256::from(123));
        assert_eq!(
            committed[&contract].storage[&U256::ZERO].present_value(),
            U256::from(1)
        );
    }
}

#[test]
fn rejected_creation_and_evm_revert_keep_their_nonce_and_fee() {
    let caller = Address::repeat_byte(0x11);
    let contract = Address::repeat_byte(0x99);
    let factory = factory();
    let fallback = StateDb::new(EvmState::from_iter([
        (caller, account(FUNDS, Bytes::new())),
        (
            contract,
            account(0, Bytes::from_static(&[0x60, 0, 0x60, 0, 0xfd])),
        ),
    ]));
    for to in [TxKind::Create, TxKind::Call(contract)] {
        let evm = factory.evm_factory().create_evm_with_inspector(
            StateDb::with_fallback(
                StateOverlay::new(EvmState::default(), StateOverlay::copy_balance_nonce),
                Some(&fallback),
            ),
            env(Address::repeat_byte(0x88)),
            Recorder::new(0),
        );
        let mut executor = factory.create_executor(evm, OpBlockExecutionCtx::default());
        let mut outcome = job(
            0,
            caller,
            to,
            123,
            Bytes::from_static(&[0x60, 1, 0x60, 0, 0x53, 0x60, 1, 0x60, 0, 0xf3]),
        )
        .run(&mut executor);
        outcome.verdict.decision = CommitDecision::Accepted;
        let executed = outcome.result_and_state().as_ref().unwrap();
        assert_eq!(executed.result.is_success(), to == TxKind::Create);
        let fee = outcome.fee;
        assert!(outcome.receipt.is_some());
        let committed = reject(outcome, &fallback);
        assert!(!committed.contains_key(&caller.create(0)));
        assert_eq!(committed[&caller].info.nonce, 1);
        assert_eq!(committed[&caller].info.balance, U256::from(FUNDS) - fee);
    }
}

#[test]
fn rejected_funding_does_not_leak_into_later_fee_balances() {
    let callers = [Address::repeat_byte(0x11), Address::repeat_byte(0x22)];
    let fallback = StateDb::new(EvmState::from_iter(
        callers.map(|caller| (caller, account(FUNDS, Bytes::new()))),
    ));
    let mut local = StateDb::with_fallback(
        StateOverlay::new(EvmState::default(), StateOverlay::copy_balance_nonce),
        Some(&fallback),
    );
    let factory = factory();
    let mut raws = Vec::new();
    for (id, caller) in callers.into_iter().enumerate() {
        let evm = factory.evm_factory().create_evm_with_inspector(
            &mut local,
            env(Address::repeat_byte(0x88)),
            Recorder::new(id as u64),
        );
        let mut executor = factory.create_executor(evm, OpBlockExecutionCtx::default());
        let mut outcome = job(
            id as u64,
            caller,
            TxKind::Call(callers[1]),
            1_000_000,
            Bytes::new(),
        )
        .run(&mut executor);
        outcome.verdict.decision = CommitDecision::Accepted;
        assert!(
            outcome
                .result_and_state()
                .as_ref()
                .unwrap()
                .result
                .is_success()
        );
        outcome.id.job = id as u64;
        outcome.verdict.decision = CommitDecision::Rejected;
        raws.push(outcome);
    }
    let committed = JobSequence::<()>::rebuild_sequence_state(&mut raws, &fallback).unwrap();
    for caller in callers {
        assert_eq!(committed[&caller].info.balance, U256::from(FUNDS - 210_000));
        assert_eq!(committed[&caller].info.nonce, 1);
    }
    assert_eq!(
        committed[&Address::repeat_byte(0x88)].info.balance,
        U256::from(42000 * 7)
    );
}

/// The planner guarantees the declared gas total stays below the block gas limit.
#[test]
fn planner_rejects_blocks_exceeding_total_gas_limit() {
    use crate::execution::ArcologyParaBlockExecutor;
    use crate::planner::Planner;

    let factory = factory();
    let callers = [Address::repeat_byte(0x11), Address::repeat_byte(0x22)];
    let fallback = StateDb::new(EvmState::from_iter(
        callers.map(|caller| (caller, account(FUNDS, Bytes::new()))),
    ));
    let mut env = env(Address::repeat_byte(0x88));
    env.block_env.gas_limit = 30_000;
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();

    for count in [1, 2] {
        let transactions = callers[..count].iter().map(|&caller| {
            let mut typed = OpTypedTransaction::Legacy(Default::default());
            let OpTypedTransaction::Legacy(tx) = &mut typed else {
                unreachable!()
            };
            tx.to = TxKind::Call(Address::repeat_byte(0x99));
            tx.gas_limit = 21_000;
            tx.gas_price = 10;
            let tx = OpTxEnvelope::new_unhashed(
                typed,
                Signature::new(U256::from(1), U256::from(1), false),
            );
            (
                ArcologyTx::from_recovered_tx(&tx, caller),
                Recovered { tx, signer: caller },
            )
        });
        let mut planner = Planner::new(StateDb::default(), Default::default());
        let result = ArcologyParaBlockExecutor::execute_block(
            &factory,
            &config,
            &pool,
            &|_, _| {},
            &fallback,
            &mut planner,
            transactions,
        )
        .map(|(outcomes, _)| {
            outcomes
                .iter()
                .filter_map(|raw| raw.gas_used())
                .sum::<u64>()
        });

        // One transaction fits; two declare 42,000 gas, over the 30,000 limit.
        match count {
            1 => assert_eq!(result.unwrap(), 21_000),
            _ => assert!(result.is_err()),
        }
    }
}

/// A single sequence must fit the gas budget across all its transactions.
#[test]
fn single_sequence_respects_total_gas_limit() {
    let caller = Address::repeat_byte(0x11);
    let fallback = StateDb::new(EvmState::from_iter([(
        caller,
        account(FUNDS, Bytes::new()),
    )]));
    let mut env = env(Address::repeat_byte(0x88));
    env.block_env.gas_limit = 30_000;
    let mut checker = ConflictDetector::new();
    checker.set_gas_limit(env.block_env.gas_limit);
    let config = ParaBlockConfig {
        env,
        ctx: OpBlockExecutionCtx::default(),
    };
    let jobs = (0..2)
        .map(|nonce| {
            let mut typed = OpTypedTransaction::Legacy(Default::default());
            let OpTypedTransaction::Legacy(tx) = &mut typed else {
                unreachable!()
            };
            tx.nonce = nonce;
            tx.to = TxKind::Call(Address::repeat_byte(0x99));
            tx.gas_limit = 21_000;
            tx.gas_price = 10;
            let tx = OpTxEnvelope::new_unhashed(
                typed,
                Signature::new(U256::from(1), U256::from(1), false),
            );
            Job::new(
                ID::new(0, nonce),
                (
                    ArcologyTx::from_recovered_tx(&tx, caller),
                    Recovered { tx, signer: caller },
                ),
            )
        })
        .collect();
    let mut generation = Generation::new(vec![JobSequence::new(jobs)]);
    generation.assign_ids(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .build()
        .unwrap();
    let (outcomes, _) = generation
        .run(&factory(), &config, &pool, &mut checker, block_state(&fallback))
        .map(|(raws, state)| (raws, state.into_inner()))
        .unwrap();

    assert_eq!(
        outcomes
            .iter()
            .filter_map(|raw| raw.gas_used())
            .sum::<u64>(),
        21_000
    );
}
