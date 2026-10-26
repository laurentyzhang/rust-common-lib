use super::ReadRecorder;
use super::recorder::MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER as RECORDER_MERGE_ENTRIES;
use super::{
    AccountDecodeError, AccountInfoCodec, ApiAddress, ArcologyBlockExecutorFactory,
    ArcologyEvmFactory, ArcologyParaBlockExecutor, ArcologyTx, CallHandler, CallHookError,
    CallInput, CallOutput, CallRouter, CallScheme, ExitStatus, ParaBlockConfig, RegisterHookError,
    ScalarHandler, SlotCodec, TransactionOutcome,
};

use crate::{
    crdt::state::status::{Status, Tag},
    execution::evm::key::{Key, KeyCodec},
    store::StoreCodec,
};
use alloy_evm::{
    Evm, EvmEnv, EvmFactory, EvmInternals, FromRecoveredTx, RecoveredTx,
    revm::{
        Journal,
        context::{BlockEnv, CfgEnv, JournalTr, TxEnv},
        context_interface::result::{
            ExecutionResult, Output, ResultAndState, ResultGas, SuccessReason,
        },
        database::{CacheDB, EmptyDB},
        inspector::NoOpInspector,
        precompile::PrecompileId,
        state::{Account, AccountInfo, Bytecode, EvmState, EvmStorageSlot},
    },
};
use alloy_op_evm::{OpBlockExecutionCtx, block::OpAlloyReceiptBuilder};
use alloy_op_hardforks::OpChainHardforks;
use alloy_primitives::{Address, Bytes, TxKind, U256};
use op_alloy::consensus::{OpTxEnvelope, TxDeposit};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use crate::scheduler::{Generation, Job, JobSequence, PassThroughScheduler, Scheduler};

// Contract bytecode that CALLs Arcology's runtime address and returns its 32-byte output.
const INTERNAL_CALLER_CODE: &[u8] = &[
    0x60, 0x20, // PUSH1 32: return size
    0x60, 0x00, // PUSH1 0: return offset
    0x60, 0x00, // PUSH1 0: input size
    0x60, 0x00, // PUSH1 0: input offset
    0x60, 0x00, // PUSH1 0: value
    0x60, 0xa0, // PUSH1 0xa0: Arcology runtime address
    0x61, 0xff, 0xff, // PUSH2 65535: forwarded gas
    0xf1, // CALL
    0x50, // POP success flag
    0x60, 0x20, // PUSH1 32: return size
    0x60, 0x00, // PUSH1 0: return offset
    0xf3, // RETURN
];

fn input(target: Address, gas_limit: u64) -> CallInput<'static> {
    CallInput {
        caller: Address::with_last_byte(0x01),
        target,
        code_address: target,
        value: U256::ZERO,
        input: &[0xde, 0xad, 0xbe, 0xef],
        gas_limit,
        scheme: CallScheme::Call,
    }
}

// Supply an isolated journal for router-only tests.
fn dispatch(
    router: &CallRouter,
    input: &CallInput<'_>,
) -> Result<Option<CallOutput>, CallHookError> {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
    router.dispatch(input, &mut internals, &mut ReadRecorder::new(0))
}

#[test]
fn evm_keys_round_trip() {
    let address = Address::repeat_byte(0x11);
    let keys = [
        Key::Account(address),
        Key::Balance(address),
        Key::Code(address),
        Key::Storage(address, U256::from(7)),
    ];

    for key in keys {
        let encoded = KeyCodec.encode(&key).unwrap();
        assert_eq!(KeyCodec.decode(&encoded).unwrap(), key);
    }
}

fn tx(caller: Address, target: Address, data: Bytes) -> ArcologyTx {
    ArcologyTx::from(TxEnv {
        caller,
        gas_limit: 500_000,
        gas_price: 0,
        kind: TxKind::Call(target),
        data,
        ..Default::default()
    })
}

#[derive(Clone)]
struct RecoveredOpTx {
    tx: OpTxEnvelope,
    signer: Address,
}

impl RecoveredTx<OpTxEnvelope> for RecoveredOpTx {
    fn tx(&self) -> &OpTxEnvelope {
        &self.tx
    }

    fn signer(&self) -> &Address {
        &self.signer
    }
}

fn factory_with_runtime_hook(
    handler: impl Fn(&CallInput<'_>) -> Result<CallOutput, CallHookError> + Send + Sync + 'static,
) -> ArcologyEvmFactory {
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), handler)
        .unwrap();
    ArcologyEvmFactory::try_new(router).unwrap()
}

#[test]
fn account_info_round_trips() {
    let code = Bytecode::new_raw(vec![0x60, 0x01, 0x00].into());
    let info = AccountInfo::new(U256::from(42), 7, code.hash_slow(), code);

    let decoded = AccountInfo::decode(&info.encode()).unwrap();

    assert_eq!(decoded.balance, info.balance);
    assert_eq!(decoded.nonce, info.nonce);
    assert_eq!(decoded.code_hash, info.code_hash);
    assert_eq!(
        decoded.code.unwrap().original_bytes(),
        info.code.unwrap().original_bytes()
    );
}

#[test]
fn malformed_account_info_is_rejected() {
    assert_eq!(AccountInfo::decode(&[]), Err(AccountDecodeError));
}

#[test]
fn normal_calls_continue() {
    let router = CallRouter::new();
    for scheme in [
        CallScheme::Call,
        CallScheme::StaticCall,
        CallScheme::CallCodeOrDelegateCall,
    ] {
        let mut call = input(Address::with_last_byte(0x42), 100);
        call.scheme = scheme;
        call.target = Address::with_last_byte(0x43);
        call.value = U256::from(1);
        assert_eq!(dispatch(&router, &call).unwrap(), None);
    }
}

#[test]
fn router_rejects_invalid_contexts_before_invoking_handlers() {
    let api = ApiAddress::Runtime.address();
    let other = Address::with_last_byte(0x22);
    let mut router = CallRouter::new();
    router
        .register(api, |_: &CallInput<'_>| {
            panic!("invalid call context reached the handler")
        })
        .unwrap();
    for (scheme, target, value) in [
        (CallScheme::CallCodeOrDelegateCall, api, U256::ZERO),
        (CallScheme::CallCodeOrDelegateCall, other, U256::ZERO),
        (CallScheme::Call, other, U256::ZERO),
        (CallScheme::StaticCall, other, U256::ZERO),
        (CallScheme::Call, api, U256::from(1)),
        (CallScheme::StaticCall, api, U256::from(1)),
    ] {
        let mut call = input(api, 100);
        call.scheme = scheme;
        call.target = target;
        call.value = value;
        assert_eq!(
            dispatch(&router, &call).unwrap(),
            Some(CallOutput::reverted(0, Vec::new()))
        );
    }
}

#[test]
fn router_allows_direct_call_and_staticcall() {
    let api = ApiAddress::Runtime.address();
    let mut router = CallRouter::new();
    router
        .register(api, |_: &CallInput<'_>| {
            Ok(CallOutput::returned(1, Bytes::from_static(b"accepted")))
        })
        .unwrap();
    for scheme in [CallScheme::Call, CallScheme::StaticCall] {
        let mut call = input(api, 100);
        call.scheme = scheme;
        assert_eq!(
            dispatch(&router, &call).unwrap(),
            Some(CallOutput::returned(1, Bytes::from_static(b"accepted")))
        );
    }
}

#[test]
fn registered_runtime_call_is_handled() {
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), |call: &CallInput<'_>| {
            assert_eq!(call.input, &[0xde, 0xad, 0xbe, 0xef]);
            Ok(CallOutput::returned(21, Bytes::from(vec![7_u8])))
        })
        .unwrap();

    let outcome = dispatch(&router, &input(ApiAddress::Runtime.address(), 100)).unwrap();

    assert_eq!(
        outcome,
        Some(CallOutput::returned(21, Bytes::from(vec![7_u8])))
    );
}

#[test]
fn router_accepts_handler_structs() {
    struct Runtime;

    impl CallHandler for Runtime {
        fn is_valid_slot(&self, _slot: U256) -> bool {
            false
        }

        fn call(
            &self,
            _key: U256,
            input: &CallInput<'_>,
            _internals: &mut EvmInternals<'_>,
            _api_reads: &mut ReadRecorder,
        ) -> Result<CallOutput, CallHookError> {
            Ok(CallOutput::returned(1, Bytes::copy_from_slice(input.input)))
        }
    }

    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), Runtime)
        .unwrap();
    let call = input(ApiAddress::Runtime.address(), 100);
    assert_eq!(
        dispatch(&router, &call).unwrap(),
        Some(CallOutput::returned(1, Bytes::copy_from_slice(call.input)))
    );
}

#[test]
fn revert_is_a_handled_result() {
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), |_: &CallInput<'_>| {
            Ok(CallOutput::reverted(
                30,
                Bytes::from_static(b"runtime revert"),
            ))
        })
        .unwrap();

    let outcome = dispatch(&router, &input(ApiAddress::Runtime.address(), 100)).unwrap();

    assert_eq!(
        outcome,
        Some(CallOutput::reverted(
            30,
            Bytes::from_static(b"runtime revert")
        ))
    );
}

#[test]
fn handler_cannot_use_more_than_supplied_gas() {
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), |_: &CallInput<'_>| {
            Ok(CallOutput::returned(101, Bytes::new()))
        })
        .unwrap();

    assert_eq!(
        dispatch(&router, &input(ApiAddress::Runtime.address(), 100)),
        Err(CallHookError::OutOfGas {
            gas_limit: 100,
            gas_used: 101,
        })
    );
}

#[test]
fn duplicate_registration_is_rejected() {
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), |_: &CallInput<'_>| {
            Ok(CallOutput::returned(11, Bytes::new()))
        })
        .unwrap();

    let result = router.register(ApiAddress::Runtime.address(), |_: &CallInput<'_>| {
        Ok(CallOutput::returned(0, Bytes::new()))
    });

    assert_eq!(
        result,
        Err(RegisterHookError {
            address: ApiAddress::Runtime.address()
        })
    );
    assert_eq!(
        dispatch(&router, &input(ApiAddress::Runtime.address(), 100)).unwrap(),
        Some(CallOutput::returned(11, Bytes::new()))
    );
}

#[test]
fn factory_installs_only_registered_arcology_precompiles() {
    let factory = factory_with_runtime_hook(|_| Ok(CallOutput::returned(1, Bytes::new())));
    let evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let (_, _, precompiles) = evm.components();

    assert!(precompiles.get(&ApiAddress::Runtime.address()).is_some());
    assert!(precompiles.get(&ApiAddress::Container.address()).is_none());
    assert!(precompiles.get(&Address::with_last_byte(0x01)).is_some());
}

#[test]
fn factory_rejects_standard_precompile_collisions() {
    let mut router = CallRouter::new();
    let ethereum_ecrecover = Address::with_last_byte(0x01);
    router
        .register(ethereum_ecrecover, |_: &CallInput<'_>| {
            Ok(CallOutput::returned(1, Bytes::new()))
        })
        .unwrap();

    let error = ArcologyEvmFactory::try_new(router).unwrap_err();
    assert_eq!(error.address, ethereum_ecrecover);
    assert_eq!(error.existing, PrecompileId::EcRec);
}

#[test]
fn factory_reports_collisions_in_address_order() {
    let mut router = CallRouter::new();
    for suffix in [0x02, 0x01] {
        router
            .register(Address::with_last_byte(suffix), |_: &CallInput<'_>| {
                Ok(CallOutput::returned(1, Bytes::new()))
            })
            .unwrap();
    }

    let error = ArcologyEvmFactory::try_new(router).unwrap_err();
    assert_eq!(error.address, Address::with_last_byte(0x01));
}

#[test]
fn direct_transaction_is_dispatched_through_revm() {
    let observed = Arc::new(Mutex::new(None));
    let handler_observed = Arc::clone(&observed);
    let factory = factory_with_runtime_hook(move |input| {
        *handler_observed.lock().unwrap() = Some((
            input.caller,
            input.target,
            input.code_address,
            input.input.to_vec(),
            input.scheme,
        ));
        Ok(CallOutput::returned(
            37,
            Bytes::from_static(b"arcology-runtime"),
        ))
    });

    let caller = Address::with_last_byte(0x11);
    let calldata = Bytes::from_static(b"pid()");
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let result = evm
        .transact(tx(caller, ApiAddress::Runtime.address(), calldata.clone()))
        .unwrap();

    assert!(result.result.is_success());
    assert_eq!(
        result.result.output(),
        Some(&Bytes::from_static(b"arcology-runtime"))
    );
    assert_eq!(
        *observed.lock().unwrap(),
        Some((
            caller,
            ApiAddress::Runtime.address(),
            ApiAddress::Runtime.address(),
            calldata.to_vec(),
            CallScheme::Call,
        ))
    );
}

#[test]
fn indirect_call_opcodes_are_rejected_before_reaching_handlers() {
    for opcode in [0xf2, 0xf4] {
        let factory = factory_with_runtime_hook(|_| panic!("indirect call reached the handler"));
        // Empty input/output buffers, followed by the optional CALLCODE value.
        let mut code = vec![0x60, 0, 0x60, 0, 0x60, 0, 0x60, 0];
        if opcode == 0xf2 {
            code.extend_from_slice(&[0x60, 0]);
        }
        // Invoke 0xa0, then return the call's success flag as a 32-byte word.
        code.extend_from_slice(&[
            0x60, 0xa0, 0x61, 0xff, 0xff, opcode, 0x60, 0, 0x52, 0x60, 0x20, 0x60, 0, 0xf3,
        ]);
        let caller = Address::with_last_byte(0x11);
        let contract = Address::with_last_byte(0x22);
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            contract,
            AccountInfo {
                code: Some(Bytecode::new_raw(code.into())),
                ..Default::default()
            },
        );
        let mut evm = factory.create_evm(db, EvmEnv::default());
        let result = evm.transact(tx(caller, contract, Bytes::new())).unwrap();
        assert!(result.result.is_success());
        assert_eq!(
            result.result.output(),
            Some(&Bytes::copy_from_slice(&[0; 32]))
        );
    }
}

#[test]
fn value_transfer_to_arcology_api_is_an_evm_revert() {
    let factory = factory_with_runtime_hook(|_| panic!("value-bearing call reached the handler"));
    let caller = Address::with_last_byte(0x11);
    let mut db = CacheDB::new(EmptyDB::default());
    db.insert_account_info(
        caller,
        AccountInfo {
            balance: U256::from(10),
            ..Default::default()
        },
    );
    let mut evm = factory.create_evm(db, EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller,
            kind: TxKind::Call(ApiAddress::Runtime.address()),
            value: U256::from(1),
            gas_limit: 500_000,
            gas_price: 0,
            ..Default::default()
        }))
        .unwrap();
    assert!(!result.result.is_success());
    assert!(!result.result.is_halt());
    assert_eq!(result.result.output(), Some(&Bytes::new()));
    assert_eq!(result.state[&caller].info.balance, U256::from(10));
}

#[test]
fn internal_call_opcode_is_dispatched_through_revm() {
    let expected = [0x7b_u8; 32];
    let factory = factory_with_runtime_hook(move |_| {
        Ok(CallOutput::returned(50, Bytes::copy_from_slice(&expected)))
    });

    let caller = Address::with_last_byte(0x11);
    let contract = Address::with_last_byte(0x22);
    let mut db = CacheDB::new(EmptyDB::default());
    db.insert_account_info(
        contract,
        AccountInfo {
            code: Some(Bytecode::new_raw(Bytes::from_static(INTERNAL_CALLER_CODE))),
            ..Default::default()
        },
    );

    let mut evm = factory.create_evm(db, EvmEnv::default());
    let result = evm.transact(tx(caller, contract, Bytes::new())).unwrap();

    assert!(result.result.is_success());
    assert_eq!(
        result.result.output(),
        Some(&Bytes::copy_from_slice(&expected))
    );
}

#[test]
fn revert_is_returned_as_an_evm_revert() {
    let factory = factory_with_runtime_hook(|_| {
        Ok(CallOutput::reverted(23, Bytes::from_static(b"rejected")))
    });
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let result = evm
        .transact(tx(
            Address::with_last_byte(0x11),
            ApiAddress::Runtime.address(),
            Bytes::new(),
        ))
        .unwrap();

    assert!(!result.result.is_success());
    assert!(!result.result.is_halt());
    assert_eq!(
        result.result.output(),
        Some(&Bytes::from_static(b"rejected"))
    );
}

#[test]
fn excessive_hook_gas_becomes_an_evm_halt() {
    let factory = factory_with_runtime_hook(|input| {
        Ok(CallOutput::returned(
            input.gas_limit.saturating_add(1),
            Bytes::new(),
        ))
    });
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let result = evm
        .transact(tx(
            Address::with_last_byte(0x11),
            ApiAddress::Runtime.address(),
            Bytes::new(),
        ))
        .unwrap();

    assert!(result.result.is_halt());
}

#[test]
fn factory_supports_standard_inspectors() {
    let factory = factory_with_runtime_hook(|_| Ok(CallOutput::returned(1, Bytes::new())));
    let _evm =
        factory.create_evm_with_inspector(EmptyDB::default(), EvmEnv::default(), NoOpInspector);
}

#[test]
fn recorder_tracks_sload_slots() {
    let caller = Address::with_last_byte(0x11);
    let contract = Address::with_last_byte(0x22);
    let slot = U256::from(7);
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0x07, 0x54, 0x50, // SLOAD slot 7, POP
        0x60, 0x07, 0x54, 0x50, // SLOAD slot 7, POP
        0x00, // STOP
    ]));
    let mut db = CacheDB::new(EmptyDB::default());
    db.insert_account_info(
        contract,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    db.insert_account_storage(contract, slot, U256::from(42))
        .unwrap();

    let factory = ArcologyEvmFactory::try_new(CallRouter::new()).unwrap();
    let mut evm = factory.create_evm_with_inspector(db, EvmEnv::default(), ReadRecorder::new(3));
    assert!(
        evm.transact(tx(caller, contract, Bytes::new()))
            .unwrap()
            .result
            .is_success()
    );

    let (_, recorder, _) = evm.components_mut();
    let reads = recorder.drain();
    let storage = reads
        .iter()
        .find(|read| read.key == Key::Storage(contract, slot))
        .unwrap();
    assert_eq!(storage.data.operations.reads.count(), 2);
}

#[test]
fn parallel_executor_keeps_native_reads_transaction_local() {
    struct SequenceScheduler;
    impl<Tx> Scheduler<Tx> for SequenceScheduler {
        fn schedule(&self, jobs: impl IntoIterator<Item = Job<Tx>>) -> Vec<Generation<Tx>> {
            let mut generation = Generation::default();
            for (index, mut job) in jobs.into_iter().enumerate() {
                let sequence = index / 3;
                job.id.sequence = sequence as u64;
                job.id.job = (index % 3) as u64;
                if generation.sequences.len() <= sequence {
                    generation.sequences.push(JobSequence {
                        jobs: Vec::new(),
                        nonce_offsets: Vec::new(),
                    });
                }
                generation.sequences[sequence].jobs.push(job);
            }
            vec![generation]
        }
    }

    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    let evm_factory = ArcologyEvmFactory::try_new(router).unwrap();
    let callers = [0x31, 0x32, 0x33, 0x34].map(Address::with_last_byte);
    for workers in [1, 4] {
        let factory = ArcologyBlockExecutorFactory::new(
            OpAlloyReceiptBuilder::default(),
            OpChainHardforks::op_mainnet(),
            evm_factory.clone(),
        );
        let executor = ArcologyParaBlockExecutor::new(
            factory,
            ParaBlockConfig {
                workers,
                env: EvmEnv::default(),
                ctx: OpBlockExecutionCtx::default(),
            },
            |_, _| {},
        );
        let transactions = (0..12).map(|index| {
            let caller = callers[index / 3];
            let input = if index % 3 == 1 {
                // The middle job reverts; no read may leak from the previous job.
                Bytes::from_static(&[0xff; 4])
            } else {
                let mut data = alloy_primitives::keccak256("get(bytes32)")[..4].to_vec();
                let handle = Address::with_last_byte(if index % 3 == 0 { 1 } else { 2 });
                let key = SlotCodec::encode(ApiAddress::Scalar, 2, handle);
                data.extend_from_slice(&key.to_be_bytes::<32>());
                data.into()
            };
            let envelope: OpTxEnvelope = TxDeposit {
                from: caller,
                to: TxKind::Call(ApiAddress::Scalar.address()),
                gas_limit: 100_000,
                input,
                ..Default::default()
            }
            .into();
            (
                ArcologyTx::from_recovered_tx(&envelope, caller),
                RecoveredOpTx {
                    tx: envelope,
                    signer: caller,
                },
            )
        });
        let results = executor.execute_block(
            &|| CacheDB::new(EmptyDB::default()),
            &SequenceScheduler,
            transactions,
        );
        assert_eq!(results.len(), 12);
        for (index, scheduled) in results.into_iter().enumerate() {
            let outcome = scheduled.outcome.unwrap();
            if index % 3 == 1 {
                assert!(matches!(
                    outcome.result_and_state.result,
                    ExecutionResult::Revert { .. }
                ));
                assert!(outcome.reads.is_empty());
                continue;
            }
            assert!(outcome.result_and_state.result.is_success());
            assert_eq!(outcome.reads.len(), 2);
            let handle = Address::with_last_byte(if index % 3 == 0 { 1 } else { 2 });
            for field in [2, 3] {
                let key = Key::Storage(
                    callers[index / 3],
                    SlotCodec::encode(ApiAddress::Scalar, field, handle),
                );
                let read = outcome.reads.iter().find(|read| read.key == key).unwrap();
                assert_eq!(read.id, index as u64);
                assert_eq!(read.data.id, index as u64);
                assert_eq!(read.data.operations.reads.count(), 1);
            }
        }
    }
}

#[test]
fn parallel_executor_preserves_sequence_state_and_reuses_workers() {
    // Two independent sequences per generation, with three ordered jobs each.
    struct SequenceScheduler;
    impl<Tx> Scheduler<Tx> for SequenceScheduler {
        fn schedule(&self, jobs: impl IntoIterator<Item = Job<Tx>>) -> Vec<Generation<Tx>> {
            let mut jobs = jobs.into_iter().peekable();
            let mut generations = Vec::<Generation<Tx>>::new();
            while jobs.peek().is_some() {
                let mut generation = Generation::default();
                for sequence in 0..2 {
                    let jobs: Vec<_> = jobs
                        .by_ref()
                        .take(3)
                        .enumerate()
                        .map(|(index, mut job)| {
                            job.id.generation = generations.len() as u64;
                            job.id.sequence = sequence;
                            job.id.job = index as u64;
                            job
                        })
                        .collect();
                    if !jobs.is_empty() {
                        generation.sequences.push(JobSequence {
                            jobs,
                            nonce_offsets: Vec::new(),
                        });
                    }
                }
                generations.push(generation);
            }
            generations
        }
    }

    let contract = Address::with_last_byte(0x22);
    let callers = [0x31, 0x32, 0x33, 0x34].map(Address::with_last_byte);
    // Increment slot zero and return it; nonempty calldata reverts the write.
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0x00, 0x54, // SLOAD(0)
        0x60, 0x01, 0x01, 0x80, // increment and DUP1
        0x60, 0x00, 0x55, // SSTORE(0, value)
        0x60, 0x00, 0x52, // MSTORE(0, value)
        0x36, 0x15, 0x60, 0x17, 0x57, // jump to success for empty calldata
        0x60, 0x00, 0x60, 0x00, 0xfd, // REVERT(0, 0)
        0x5b, 0x60, 0x20, 0x60, 0x00, 0xf3, // return the value
    ]));
    let transaction = |caller, input| {
        let envelope: OpTxEnvelope = TxDeposit {
            from: caller,
            to: TxKind::Call(contract),
            gas_limit: 100_000,
            input,
            ..Default::default()
        }
        .into();
        (
            ArcologyTx::from_recovered_tx(&envelope, caller),
            RecoveredOpTx {
                tx: envelope,
                signer: caller,
            },
        )
    };

    for workers in [0, 1, 2, 4] {
        let databases = AtomicUsize::new(0);
        let threads = Mutex::new(HashSet::new());
        let make_db = || {
            databases.fetch_add(1, Ordering::Relaxed);
            threads.lock().unwrap().insert(std::thread::current().id());
            let mut db = CacheDB::new(EmptyDB::default());
            db.insert_account_info(
                contract,
                AccountInfo {
                    code_hash: code.hash_slow(),
                    code: Some(code.clone()),
                    ..Default::default()
                },
            );
            for caller in callers {
                db.insert_account_info(
                    caller,
                    AccountInfo {
                        nonce: 7,
                        ..Default::default()
                    },
                );
            }
            db
        };
        let factory = ArcologyBlockExecutorFactory::new(
            OpAlloyReceiptBuilder::default(),
            OpChainHardforks::op_mainnet(),
            ArcologyEvmFactory::try_new(CallRouter::new()).unwrap(),
        );
        let executor = ArcologyParaBlockExecutor::new(
            factory,
            ParaBlockConfig {
                workers,
                env: EvmEnv::default(),
                ctx: OpBlockExecutionCtx::default(),
            },
            |_, _| {},
        );

        // Reusing the executor across blocks must reuse the same bounded worker set.
        for block in 0..2 {
            let results = executor.execute_block(
                &make_db,
                &SequenceScheduler,
                (0..12).map(|index| transaction(callers[index / 3], Bytes::new())),
            );
            assert_eq!(results.len(), 12);
            assert_eq!(databases.load(Ordering::Relaxed), (block + 1) * 4);
            for (index, scheduled) in results.into_iter().enumerate() {
                let position = index % 3;
                assert_eq!(scheduled.id.global, index as u64);
                assert_eq!(scheduled.id.generation, (index / 6) as u64);
                assert_eq!(scheduled.id.sequence, ((index % 6) / 3) as u64);
                assert_eq!(scheduled.id.job, position as u64);
                let outcome = scheduled.outcome.unwrap();
                let result = &outcome.result_and_state;
                assert!(result.result.is_success());
                assert_eq!(
                    U256::from_be_slice(result.result.output().unwrap()),
                    U256::from(position + 1)
                );
                let slot = &result.state[&contract].storage[&U256::ZERO];
                assert_eq!(slot.original_value(), U256::from(position));
                assert_eq!(slot.present_value(), U256::from(position + 1));
                assert_eq!(
                    result.state[&callers[index / 3]].info.nonce,
                    8 + position as u64
                );
                assert!(
                    outcome
                        .reads
                        .iter()
                        .all(|read| read.id == index as u64 && read.data.id == index as u64)
                );
                let storage_read = outcome
                    .reads
                    .iter()
                    .find(|read| read.key == Key::Storage(contract, U256::ZERO))
                    .unwrap();
                assert_eq!(storage_read.data.operations.reads.count(), 1);
            }
        }
        assert!(threads.lock().unwrap().len() <= workers.max(1));

        // Separate snapshots start at the sender nonce plus their scheduler offset.
        let results = executor.execute_block(
            &make_db,
            &PassThroughScheduler::new(),
            (0..3).map(|_| transaction(callers[0], Bytes::new())),
        );
        for (index, scheduled) in results.into_iter().enumerate() {
            let outcome = scheduled.outcome.unwrap();
            assert_eq!(
                outcome.result_and_state.state[&callers[0]].info.nonce,
                8 + index as u64
            );
        }

        // A revert advances the sender nonce but its storage write is rolled back.
        let results = executor.execute_block(
            &make_db,
            &SequenceScheduler,
            [
                transaction(callers[0], Bytes::from_static(&[1])),
                transaction(callers[0], Bytes::new()),
            ],
        );
        assert!(matches!(
            results[0].outcome.as_ref().unwrap().result_and_state.result,
            ExecutionResult::Revert { .. }
        ));
        let outcome = results[1].outcome.as_ref().unwrap();
        assert_eq!(
            U256::from_be_slice(outcome.result_and_state.result.output().unwrap()),
            U256::from(1)
        );
        assert_eq!(outcome.result_and_state.state[&callers[0]].info.nonce, 9);
        assert!(
            outcome
                .reads
                .iter()
                .all(|read| read.id == 1 && read.data.id == 1)
        );

        let before = databases.load(Ordering::Relaxed);
        assert!(
            executor
                .execute_block(
                    &make_db,
                    &SequenceScheduler,
                    std::iter::empty::<(ArcologyTx, RecoveredOpTx)>()
                )
                .is_empty()
        );
        assert_eq!(databases.load(Ordering::Relaxed), before);
    }
}

#[test]
fn parallel_executor_returns_contract_outcome() {
    let caller = Address::with_last_byte(0x11);
    let contract = Address::with_last_byte(0x22);
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0x2a, // PUSH1 42
        0x60, 0x00, // PUSH1 0
        0x55, // SSTORE
        0x60, 0x2a, // PUSH1 42
        0x60, 0x00, // PUSH1 0
        0x52, // MSTORE
        0x60, 0x20, // PUSH1 32
        0x60, 0x00, // PUSH1 0
        0xf3, // RETURN
    ]));

    let make_db = || {
        let mut db = CacheDB::new(EmptyDB::default());
        db.insert_account_info(
            contract,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code.clone()),
                ..Default::default()
            },
        );
        db
    };

    let evm_factory = ArcologyEvmFactory::try_new(CallRouter::new()).unwrap();
    let factory = ArcologyBlockExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        evm_factory,
    );
    let executor = ArcologyParaBlockExecutor::new(
        factory,
        ParaBlockConfig {
            workers: 1,
            env: EvmEnv::default(),
            ctx: OpBlockExecutionCtx::default(),
        },
        |_, _| {},
    );

    let envelope: OpTxEnvelope = TxDeposit {
        from: caller,
        to: TxKind::Call(contract),
        gas_limit: 100_000,
        ..Default::default()
    }
    .into();
    let tx_env = ArcologyTx::from_recovered_tx(&envelope, caller);
    let recovered = RecoveredOpTx {
        tx: envelope,
        signer: caller,
    };

    let mut results = executor.execute_block(
        &make_db,
        &PassThroughScheduler::new(),
        [(tx_env, recovered)],
    );
    let scheduled = results.pop().unwrap();
    let transaction_id = scheduled.id.global;
    let mut outcome = scheduled.outcome.unwrap();
    let conflict_records = outcome.merge_records(transaction_id);
    let mut expected = [0_u8; 32];
    expected[31] = 42;

    assert_eq!(scheduled.id.global, 0);
    assert!(outcome.result_and_state.result.is_success());
    assert_eq!(
        outcome.result_and_state.result.output(),
        Some(&Bytes::copy_from_slice(&expected))
    );
    assert!(!outcome.result_and_state.state.is_empty());
    let storage_write = conflict_records
        .iter()
        .find(|(key, _)| *key == Key::Storage(contract, U256::ZERO))
        .map(|(_, tracked)| tracked)
        .unwrap();
    assert!(matches!(
        storage_write.original(),
        Status::Tag(Tag::Missing)
    ));
    assert!(matches!(
        storage_write.current(),
        Status::Tag(Tag::Stripped)
    ));
    assert_eq!(storage_write.operations.deltas.count(), 1);
    assert!(
        outcome
            .result_and_state
            .state
            .get(&contract)
            .unwrap()
            .storage
            .contains_key(&U256::ZERO)
    );
    let account = conflict_records
        .iter()
        .find(|(key, _)| *key == Key::Account(contract))
        .map(|(_, tracked)| tracked)
        .unwrap();
    assert!(account.operations.reads.count() > 0);
    assert_eq!(account.operations.deltas.count(), 0);
}

#[test]
fn conflict_records_tag_zeroed_storage_as_deleted() {
    let caller = Address::with_last_byte(0x11);
    let contract = Address::with_last_byte(0x22);
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x60, 0x00, // PUSH1 0: value
        0x60, 0x00, // PUSH1 0: slot
        0x55, // SSTORE
        0x00, // STOP
    ]));
    let mut db = CacheDB::new(EmptyDB::default());
    db.insert_account_info(
        contract,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    db.insert_account_storage(contract, U256::ZERO, U256::from(9))
        .unwrap();

    let factory = ArcologyEvmFactory::try_new(CallRouter::new()).unwrap();
    let mut evm = factory.create_evm(db, EvmEnv::default());
    let result_and_state = evm.transact(tx(caller, contract, Bytes::new())).unwrap();
    let mut outcome = TransactionOutcome {
        result_and_state,
        receipt: (),
        reads: Vec::new(),
    };
    let records = outcome.merge_records(3);
    let storage = records
        .iter()
        .find(|(key, _)| *key == Key::Storage(contract, U256::ZERO))
        .map(|(_, tracked)| tracked)
        .unwrap();

    assert!(matches!(storage.original(), Status::Tag(Tag::Stripped)));
    assert!(matches!(storage.current(), Status::Tag(Tag::Deleted)));
    assert_eq!(storage.operations.removes.count(), 1);
    assert_eq!(
        outcome.result_and_state.state[&contract].storage[&U256::ZERO].present_value(),
        U256::ZERO
    );
}

#[test]
fn conflict_records_include_untouched_account_and_slot_as_read_only() {
    let address = Address::with_last_byte(0x33);
    let slot = U256::from(4);
    let mut account = Account::from(AccountInfo {
        balance: U256::from(5),
        ..Default::default()
    });
    account
        .storage
        .insert(slot, EvmStorageSlot::new(U256::from(9), Default::default()));
    let state = EvmState::from_iter([(address, account)]);
    let mut outcome = TransactionOutcome {
        result_and_state: result_and_state(state),
        receipt: (),
        reads: Vec::new(),
    };

    let records = outcome.merge_records(7);
    for key in [Key::Account(address), Key::Storage(address, slot)] {
        let record = records
            .iter()
            .find(|(record_key, _)| *record_key == key)
            .map(|(_, tracked)| tracked)
            .unwrap();
        assert!(matches!(record.original(), Status::Tag(Tag::Stripped)));
        assert!(matches!(record.current(), Status::Tag(Tag::Missing)));
        assert_eq!(record.operations.reads.count(), 0);
        assert_eq!(record.operations.deltas.count(), 0);
        assert_eq!(record.operations.removes.count(), 0);
    }
    assert_eq!(
        outcome.result_and_state.state[&address].storage[&slot].present_value(),
        U256::from(9)
    );
}

#[test]
fn conflict_records_tag_existing_account_update_as_stripped() {
    let address = Address::with_last_byte(0x44);
    let mut account = Account::from(AccountInfo {
        balance: U256::from(5),
        ..Default::default()
    });
    account.info.balance = U256::from(6);
    account.mark_touch();
    let state = EvmState::from_iter([(address, account)]);
    let mut outcome = TransactionOutcome {
        result_and_state: result_and_state(state),
        receipt: (),
        reads: Vec::new(),
    };
    let records = outcome.merge_records(8);
    let record = records
        .iter()
        .find(|(key, _)| *key == Key::Account(address))
        .map(|(_, tracked)| tracked)
        .unwrap();

    assert!(matches!(record.original(), Status::Tag(Tag::Stripped)));
    assert!(matches!(record.current(), Status::Tag(Tag::Stripped)));
    assert_eq!(record.operations.deltas.count(), 1);
}

#[test]
fn conflict_records_use_changes_instead_of_touch_status() {
    let empty_address = Address::with_last_byte(0x45);
    let changed_address = Address::with_last_byte(0x46);
    let slot = U256::from(2);

    let mut empty = Account::new_not_existing(Default::default());
    empty.mark_touch();

    let mut changed = Account::from(AccountInfo {
        balance: U256::from(5),
        ..Default::default()
    });
    changed.info.balance = U256::from(6);
    changed.storage.insert(
        slot,
        EvmStorageSlot::new_changed(U256::from(1), U256::from(2), Default::default()),
    );

    let state = EvmState::from_iter([(empty_address, empty), (changed_address, changed)]);
    let mut outcome = TransactionOutcome {
        result_and_state: result_and_state(state),
        receipt: (),
        reads: Vec::new(),
    };
    let records = outcome.merge_records(9);
    let empty_record = &records
        .iter()
        .find(|(key, _)| *key == Key::Account(empty_address))
        .unwrap()
        .1;
    assert!(matches!(empty_record.original(), Status::Tag(Tag::Missing)));
    assert!(matches!(empty_record.current(), Status::Tag(Tag::Missing)));
    assert_eq!(empty_record.operations.deltas.count(), 0);

    for key in [
        Key::Account(changed_address),
        Key::Storage(changed_address, slot),
    ] {
        let record = &records
            .iter()
            .find(|(record_key, _)| *record_key == key)
            .unwrap()
            .1;
        assert!(matches!(record.original(), Status::Tag(Tag::Stripped)));
        assert!(matches!(record.current(), Status::Tag(Tag::Stripped)));
        assert_eq!(record.operations.deltas.count(), 1);
    }
}

#[test]
fn hook_output_status_is_preserved() {
    assert_eq!(
        CallOutput::returned(1, Bytes::new()).status,
        ExitStatus::Return
    );
    assert_eq!(
        CallOutput::reverted(1, Bytes::new()).status,
        ExitStatus::Revert
    );
}

#[test]
fn recorder_merge_sums_reads_for_the_same_key() {
    let shared = Key::Balance(Address::with_last_byte(1));
    let mut left = ReadRecorder::new(7);
    left.record(shared.clone());
    left.record(shared.clone());
    left.record(Key::Code(Address::with_last_byte(2)));

    let mut right = ReadRecorder::new(7);
    right.record(shared);
    right.record(Key::Code(Address::with_last_byte(3)));

    left.merge(right);
    let reads = left.drain();

    assert_eq!(reads.len(), 3);
    assert_eq!(
        reads
            .iter()
            .find(|read| read.key == Key::Account(Address::with_last_byte(1)))
            .unwrap()
            .data
            .operations
            .reads
            .count(),
        3
    );
}

#[test]
fn recorder_drain_groups_balance_and_code_by_account() {
    let address = Address::with_last_byte(1);
    let mut recorder = ReadRecorder::new(7);
    recorder.record(Key::Balance(address));
    recorder.record(Key::Code(address));

    let reads = recorder.drain();

    assert_eq!(reads.len(), 1);
    assert_eq!(reads[0].key, Key::Account(address));
    assert_eq!(reads[0].data.operations.reads.count(), 2);
}

#[test]
fn recorder_merge_all_uses_parallel_tree_for_large_recorders() {
    let recorders: Vec<_> = (0..4)
        .map(|recorder_index| {
            let mut recorder = ReadRecorder::new(9);
            let first_slot = recorder_index * RECORDER_MERGE_ENTRIES;

            for slot in first_slot..first_slot + RECORDER_MERGE_ENTRIES {
                recorder.record(Key::Storage(Address::ZERO, U256::from(slot)));
            }

            recorder
        })
        .collect();

    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(2)
        .build()
        .unwrap();
    let mut merged = ReadRecorder::new(9);
    pool.install(|| {
        assert!(ReadRecorder::should_merge_parallel(&recorders));
        merged.merge_all(recorders);
    });

    assert_eq!(merged.drain().len(), 4 * RECORDER_MERGE_ENTRIES);
}

fn result_and_state(state: EvmState) -> ResultAndState {
    ResultAndState::new(
        ExecutionResult::Success {
            reason: SuccessReason::Stop,
            gas: ResultGas::default(),
            logs: Vec::new(),
            output: Output::Call(Bytes::new()),
        },
        state,
    )
}
