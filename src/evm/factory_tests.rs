use super::*;
use crate::evm::{
    ArcologyApi, CallHookInput, CallHookOutput, ClosureCallHook, ExitStatus, arcology_address,
};
use alloy_evm::{
    Evm,
    revm::{
        context::TxEnv,
        database::{CacheDB, EmptyDB},
        state::{AccountInfo, Bytecode},
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

fn tx(caller: Address, target: Address, data: Bytes) -> ArcologyTx {
    // Tests use raw TxEnv values; production block execution normally converts recovered txs.
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
    handler: impl Fn(&CallHookInput<'_>) -> Result<CallHookOutput, super::super::CallHookError>
    + Send
    + Sync
    + 'static,
) -> ArcologyEvmFactory {
    // Register only the runtime hook so tests can verify selective installation.
    let mut router = CallHookRouter::new();
    router
        .register(ArcologyApi::Runtime, ClosureCallHook::new(handler))
        .unwrap();
    ArcologyEvmFactory::try_new(router).unwrap()
}

#[test]
fn factory_installs_only_registered_arcology_precompiles() {
    let factory = factory_with_runtime_hook(|_| Ok(CallHookOutput::returned(1, Bytes::new())));
    let evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let (_, _, precompiles) = evm.components();

    // OP precompiles remain present alongside the one registered Arcology hook.
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
            super::super::CallScheme::Call,
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
    assert_eq!(U256::ZERO, U256::ZERO);
}
