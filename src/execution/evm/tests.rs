use super::inspector::MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER as RECORDER_MERGE_ENTRIES;
use super::inspector::StateReadRecorder;
use super::{
    AccountDecodeError, AccountInfoCodec, ArcologyApi, ArcologyBlockExecutorFactory,
    ArcologyEvmFactory, ArcologyParaBlockExecutor, ArcologyTx, CallHookError, CallHookInput,
    CallHookOutcome, CallHookOutput, CallHookRouter, CallScheme, ClosureCallHook, ExitStatus,
    ParaBlockConfig, RegisterHookError, TransactionOutcome, arcology_address, delta_records,
};

use crate::{
    crdt::state::status::{Status, Tag},
    execution::evm::key::{Key, KeyCodec},
    store::StoreCodec,
};
use alloy_evm::{
    Evm, EvmEnv, EvmFactory, FromRecoveredTx, RecoveredTx,
    revm::{
        context::TxEnv,
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
use std::sync::{Arc, Mutex};

use crate::scheduler::PassThroughScheduler;

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

fn input(target: Address, gas_limit: u64) -> CallHookInput<'static> {
    CallHookInput {
        caller: arcology_address(0x01),
        target,
        code_address: target,
        value: U256::ZERO,
        input: &[0xde, 0xad, 0xbe, 0xef],
        gas_limit,
        scheme: CallScheme::Call,
    }
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
    handler: impl Fn(&CallHookInput<'_>) -> Result<CallHookOutput, CallHookError>
    + Send
    + Sync
    + 'static,
) -> ArcologyEvmFactory {
    let mut router = CallHookRouter::new();
    router
        .register(ArcologyApi::Runtime, ClosureCallHook::new(handler))
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
fn recognizes_arcology_addresses() {
    for api in ArcologyApi::ALL {
        assert_eq!(ArcologyApi::from_address(api.address()), Some(api));
    }

    assert_eq!(ArcologyApi::from_address(arcology_address(0x42)), None);
}

#[test]
fn normal_calls_continue() {
    let router = CallHookRouter::new();
    let outcome = router
        .dispatch(&input(arcology_address(0x42), 100))
        .unwrap();

    assert_eq!(outcome, CallHookOutcome::Continue);
}

#[test]
fn registered_runtime_call_is_handled() {
    let mut router = CallHookRouter::new();
    router
        .register(
            ArcologyApi::Runtime,
            ClosureCallHook::new(|call: &CallHookInput<'_>| {
                assert_eq!(call.input, &[0xde, 0xad, 0xbe, 0xef]);
                Ok(CallHookOutput::returned(21, Bytes::from(vec![7_u8])))
            }),
        )
        .unwrap();

    let outcome = router
        .dispatch(&input(ArcologyApi::Runtime.address(), 100))
        .unwrap();

    assert_eq!(
        outcome,
        CallHookOutcome::Handled(CallHookOutput::returned(21, Bytes::from(vec![7_u8])))
    );
}

#[test]
fn revert_is_a_handled_result() {
    let mut router = CallHookRouter::new();
    router
        .register(
            ArcologyApi::Runtime,
            ClosureCallHook::new(|_: &CallHookInput<'_>| {
                Ok(CallHookOutput::reverted(
                    30,
                    Bytes::from_static(b"runtime revert"),
                ))
            }),
        )
        .unwrap();

    let outcome = router
        .dispatch(&input(ArcologyApi::Runtime.address(), 100))
        .unwrap();

    assert_eq!(
        outcome,
        CallHookOutcome::Handled(CallHookOutput::reverted(
            30,
            Bytes::from_static(b"runtime revert")
        ))
    );
}

#[test]
fn handler_cannot_use_more_than_supplied_gas() {
    let mut router = CallHookRouter::new();
    router
        .register(
            ArcologyApi::Runtime,
            ClosureCallHook::new(|_: &CallHookInput<'_>| {
                Ok(CallHookOutput::returned(101, Bytes::new()))
            }),
        )
        .unwrap();

    assert_eq!(
        router.dispatch(&input(ArcologyApi::Runtime.address(), 100)),
        Err(CallHookError::OutOfGas {
            gas_limit: 100,
            gas_used: 101,
        })
    );
}

#[test]
fn duplicate_registration_is_rejected() {
    let mut router = CallHookRouter::new();
    router
        .register(
            ArcologyApi::Runtime,
            ClosureCallHook::new(|_: &CallHookInput<'_>| {
                Ok(CallHookOutput::returned(0, Bytes::new()))
            }),
        )
        .unwrap();

    let result = router.register(
        ArcologyApi::Runtime,
        ClosureCallHook::new(|_: &CallHookInput<'_>| Ok(CallHookOutput::returned(0, Bytes::new()))),
    );

    assert_eq!(
        result,
        Err(RegisterHookError {
            address: ArcologyApi::Runtime.address()
        })
    );
}

#[test]
fn factory_installs_only_registered_arcology_precompiles() {
    let factory = factory_with_runtime_hook(|_| Ok(CallHookOutput::returned(1, Bytes::new())));
    let evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let (_, _, precompiles) = evm.components();

    assert!(precompiles.get(&ArcologyApi::Runtime.address()).is_some());
    assert!(precompiles.get(&ArcologyApi::Container.address()).is_none());
    assert!(precompiles.get(&arcology_address(0x01)).is_some());
}

#[test]
fn factory_rejects_standard_precompile_collisions() {
    let mut router = CallHookRouter::new();
    let ethereum_ecrecover = arcology_address(0x01);
    router
        .register_address(
            ethereum_ecrecover,
            ClosureCallHook::new(|_: &CallHookInput<'_>| {
                Ok(CallHookOutput::returned(1, Bytes::new()))
            }),
        )
        .unwrap();

    let error = ArcologyEvmFactory::try_new(router).unwrap_err();
    assert_eq!(error.address, ethereum_ecrecover);
    assert_eq!(error.existing, PrecompileId::EcRec);
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
        Ok(CallHookOutput::returned(
            37,
            Bytes::from_static(b"arcology-runtime"),
        ))
    });

    let caller = arcology_address(0x11);
    let calldata = Bytes::from_static(b"pid()");
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let result = evm
        .transact(tx(caller, ArcologyApi::Runtime.address(), calldata.clone()))
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
            ArcologyApi::Runtime.address(),
            ArcologyApi::Runtime.address(),
            calldata.to_vec(),
            CallScheme::Call,
        ))
    );
}

#[test]
fn internal_call_opcode_is_dispatched_through_revm() {
    let expected = [0x7b_u8; 32];
    let factory = factory_with_runtime_hook(move |_| {
        Ok(CallHookOutput::returned(
            50,
            Bytes::copy_from_slice(&expected),
        ))
    });

    let caller = arcology_address(0x11);
    let contract = arcology_address(0x22);
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
        Ok(CallHookOutput::reverted(
            23,
            Bytes::from_static(b"rejected"),
        ))
    });
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let result = evm
        .transact(tx(
            arcology_address(0x11),
            ArcologyApi::Runtime.address(),
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
        Ok(CallHookOutput::returned(
            input.gas_limit.saturating_add(1),
            Bytes::new(),
        ))
    });
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let result = evm
        .transact(tx(
            arcology_address(0x11),
            ArcologyApi::Runtime.address(),
            Bytes::new(),
        ))
        .unwrap();

    assert!(result.result.is_halt());
}

#[test]
fn factory_supports_standard_inspectors() {
    let factory = factory_with_runtime_hook(|_| Ok(CallHookOutput::returned(1, Bytes::new())));
    let _evm =
        factory.create_evm_with_inspector(EmptyDB::default(), EvmEnv::default(), NoOpInspector);
}

#[test]
fn recorder_tracks_sload_slots() {
    let caller = arcology_address(0x11);
    let contract = arcology_address(0x22);
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

    let factory = ArcologyEvmFactory::try_new(CallHookRouter::new()).unwrap();
    let mut evm =
        factory.create_evm_with_inspector(db, EvmEnv::default(), StateReadRecorder::new(3));
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
fn parallel_executor_returns_contract_outcome() {
    let caller = arcology_address(0x11);
    let contract = arcology_address(0x22);
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

    let evm_factory = ArcologyEvmFactory::try_new(CallHookRouter::new()).unwrap();
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
    let caller = arcology_address(0x11);
    let contract = arcology_address(0x22);
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

    let factory = ArcologyEvmFactory::try_new(CallHookRouter::new()).unwrap();
    let mut evm = factory.create_evm(db, EvmEnv::default());
    let result_and_state = evm.transact(tx(caller, contract, Bytes::new())).unwrap();
    let mut outcome = TransactionOutcome {
        result_and_state,
        receipt: (),
        inspector_reads: Vec::new(),
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
    let address = arcology_address(0x33);
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
        inspector_reads: Vec::new(),
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
    let address = arcology_address(0x44);
    let mut account = Account::from(AccountInfo {
        balance: U256::from(5),
        ..Default::default()
    });
    account.info.balance = U256::from(6);
    account.mark_touch();
    let state = EvmState::from_iter([(address, account)]);
    let records = delta_records(&result_and_state(state), 8);
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
    let empty_address = arcology_address(0x45);
    let changed_address = arcology_address(0x46);
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
    let records = delta_records(&result_and_state(state), 9);
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
        CallHookOutput::returned(1, Bytes::new()).status,
        ExitStatus::Return
    );
    assert_eq!(
        CallHookOutput::reverted(1, Bytes::new()).status,
        ExitStatus::Revert
    );
}

#[test]
fn recorder_merge_sums_reads_for_the_same_key() {
    let shared = Key::Balance(Address::with_last_byte(1));
    let mut left = StateReadRecorder::new(7);
    left.record(shared.clone());
    left.record(shared.clone());
    left.record(Key::Code(Address::with_last_byte(2)));

    let mut right = StateReadRecorder::new(7);
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
    let mut recorder = StateReadRecorder::new(7);
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
            let mut recorder = StateReadRecorder::new(9);
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
    let mut merged = StateReadRecorder::new(9);
    pool.install(|| {
        assert!(StateReadRecorder::should_merge_parallel(&recorders));
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
