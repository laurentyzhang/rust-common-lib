use super::*;
use crate::eu::{ArcologyBlockExecutorFactory, ArcologyTx, CallRouter, StateDb};
use alloy_evm::{
    FromRecoveredTx, RecoveredTx,
    revm::state::{Account, AccountInfo, Bytecode, EvmState},
};
use alloy_op_evm::block::OpAlloyReceiptBuilder;
use alloy_op_hardforks::OpChainHardforks;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use op_alloy::consensus::{OpTxEnvelope, TxDeposit};
use op_revm::OpSpecId;
use std::collections::BTreeSet;

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
                    db: (),
                    jobs: vec![job(id, contract)],
                    nonce_offsets: Vec::new(),
                })
                .collect(),
        };
        let (outcomes, result) = ArcologyParaBlockExecutor::run_generation(
            &factory,
            &config,
            &pool,
            &|state| StateDb::new(state.clone()),
            &mut committer,
            generation,
            &mut checker,
        )
        .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(
            outcomes
                .iter()
                .all(|outcome| outcome.result_and_state.result.is_success())
        );
        assert_eq!(result.committable, BTreeSet::from([1]));
        assert_eq!(result.conflicts, BTreeSet::from([(1, 2)]));
        assert_eq!(
            committer.inner()[&contract].storage[&U256::ZERO].present_value(),
            U256::from(1)
        );
        assert!(
            !committer
                .inner()
                .contains_key(&Address::with_last_byte(0x12))
        );
        assert!(
            result
                .rejected
                .iter()
                .any(|(record, _)| record.id.global == 2
                    && record.key == Key::Storage(contract, U256::ZERO))
        );
        assert!(
            result
                .rejected
                .iter()
                .all(|(record, _)| record.gas_used > 0)
        );

        let generation = Generation {
            sequences: vec![JobSequence {
                db: (),
                jobs: vec![job(3, contract)],
                nonce_offsets: Vec::new(),
            }],
        };
        let (outcomes, result) = ArcologyParaBlockExecutor::run_generation(
            &factory,
            &config,
            &pool,
            &|state| StateDb::new(state.clone()),
            &mut committer,
            generation,
            &mut checker,
        )
        .unwrap();
        assert_eq!(outcomes.len(), 1);
        assert_eq!(result.committable, BTreeSet::from([3]));
        assert!(result.rejected.is_empty());
        assert!(result.conflicts.is_empty());
        assert_eq!(
            outcomes[0].result_and_state.state[&contract].storage[&U256::ZERO].present_value(),
            U256::from(2)
        );
        assert_eq!(
            committer.inner()[&contract].storage[&U256::ZERO].present_value(),
            U256::from(2)
        );

        let mut planner = Planner::new(StateDb::new(state.clone()), Default::default());
        let mut committer = StatDbCommitter::with_state(state.clone());
        let (outcomes, resolutions) = ArcologyParaBlockExecutor::execute_block(
            &factory,
            &config,
            &pool,
            &|_, _| {},
            &|state| StateDb::new(state.clone()),
            &mut planner,
            &mut committer,
            [job(1, contract).tx, job(2, contract).tx],
        )
        .unwrap();
        assert_eq!(outcomes.len(), 2);
        assert_eq!(resolutions.len(), 1);
        assert_eq!(resolutions[0].committable, BTreeSet::from([0]));
        assert_eq!(resolutions[0].conflicts, BTreeSet::from([(0, 1)]));
        let count = crate::planner::history::Field::Count.slot(0);
        assert_eq!(
            resolutions[0].history[&contract].storage[&count].present_value(),
            U256::from(1)
        );
        assert_eq!(planner.db.storage(contract, count).unwrap(), U256::ZERO);
    }
}
