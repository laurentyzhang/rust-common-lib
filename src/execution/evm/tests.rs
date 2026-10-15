use super::{
    AccountDecodeError, AccountInfoCodec, ArcologyApi, ArcologyEvmFactory, ArcologyTx,
    CallHookError, CallHookInput, CallHookOutcome, CallHookOutput, CallHookRouter, CallScheme,
    ClosureCallHook, ExitStatus, RegisterHookError, StateDB, arcology_address,
};
use crate::execution::VmStateCache;
use alloy_evm::{
    Evm, EvmEnv, EvmFactory,
    revm::{
        Database, DatabaseCommit,
        context::TxEnv,
        database::{CacheDB, EmptyDB},
        inspector::NoOpInspector,
        precompile::PrecompileId,
        state::{Account, AccountInfo, Bytecode, EvmState, EvmStorageSlot},
    },
};
use alloy_primitives::{Address, Bytes, TxKind, U256};
use std::sync::{Arc, Mutex};

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
fn commit_writes_account_and_storage_to_vm_cache() {
    let address = Address::repeat_byte(0x11);
    let slot = U256::from(7);
    let storage_value = U256::from(42);
    let info = AccountInfo {
        balance: U256::from(100),
        nonce: 3,
        ..AccountInfo::default()
    };
    let account = Account::from(info.clone())
        .with_storage(
            [(
                slot,
                EvmStorageSlot::new_changed(U256::ZERO, storage_value, Default::default()),
            )]
            .into_iter(),
        )
        .with_touched_mark();
    let mut changes = EvmState::default();
    changes.insert(address, account);

    let mut db = StateDB::new(VmStateCache::new(9));
    db.commit(changes);

    assert_eq!(db.execution_store().id(), 9);
    assert_eq!(db.basic(address).unwrap(), Some(info));
    assert_eq!(db.storage(address, slot).unwrap(), storage_value);
    assert_eq!(db.execution_store().drain().1.len(), 2);
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
