use super::super::version::VERSION;
use super::{
    CallHandler, ScalarHandler,
    scalar::{IScalar, Slot},
};
use crate::crdt::{self, state::DeltaOp};
use crate::execution::evm::{
    ApiAddress, ArcologyEvmFactory, ArcologyTx, CallInput, CallOutput, CallRouter, CallScheme,
    ExitStatus, ReadRecorder, SlotCodec, key::Key,
};
use alloy_evm::{
    Evm, EvmEnv, EvmFactory, EvmInternals,
    revm::{
        Journal,
        context::{BlockEnv, CfgEnv, JournalTr, TxEnv},
        database::{EmptyDB, InMemoryDB},
        state::{AccountInfo, Bytecode},
    },
};
use alloy_primitives::{Address, B256, TxKind, U256};
use alloy_sol_types::{SolCall, SolValue};

fn invoke(
    journal: &mut Journal<EmptyDB>,
    owner: Address,
    call: &impl SolCall,
    scheme: CallScheme,
    value: U256,
) -> CallOutput {
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    let data = call.abi_encode();
    let input = CallInput {
        caller: owner,
        target: ApiAddress::Scalar.address(),
        code_address: ApiAddress::Scalar.address(),
        value,
        input: &data,
        gas_limit: 500_000,
        scheme,
    };
    let mut internals = EvmInternals::new(journal, &block, &cfg, &tx);
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    router
        .dispatch(&input, &mut internals, &mut ReadRecorder::new(0))
        .unwrap()
        .unwrap()
}

fn scalar_key(handle: Address) -> B256 {
    SlotCodec::encode(ApiAddress::Scalar, 2, handle).into()
}

fn slots(handle: Address) -> Slot {
    Slot {
        min: SlotCodec::encode(ApiAddress::Scalar, 0, handle),
        max: SlotCodec::encode(ApiAddress::Scalar, 1, handle),
        value: SlotCodec::encode(ApiAddress::Scalar, 2, handle),
        delta: SlotCodec::encode(ApiAddress::Scalar, 3, handle),
    }
}

fn invoke_delta(
    journal: &mut Journal<EmptyDB>,
    owner: Address,
    handle: Address,
    delta: DeltaOp<U256>,
    scheme: CallScheme,
    value: U256,
) -> CallOutput {
    match delta {
        DeltaOp::Add(delta) => invoke(
            journal,
            owner,
            &IScalar::addCall {
                key: scalar_key(handle),
                delta,
            },
            scheme,
            value,
        ),
        DeltaOp::Sub(delta) => invoke(
            journal,
            owner,
            &IScalar::subCall {
                key: scalar_key(handle),
                delta,
            },
            scheme,
            value,
        ),
    }
}

fn stored_values(journal: &mut Journal<EmptyDB>, owner: Address, keys: Slot) -> [U256; 4] {
    [keys.min, keys.max, keys.value, keys.delta].map(|key| journal.sload(owner, key).unwrap().data)
}

#[test]
fn scalar_handler_recognizes_slots_through_the_trait() {
    use strum::IntoEnumIterator;

    let handler: &dyn CallHandler = &ScalarHandler;
    for handle in [Address::ZERO, Address::repeat_byte(0xff)] {
        let keys = slots(handle);
        for slot in [keys.min, keys.max, keys.value, keys.delta] {
            assert!(handler.is_valid_slot(slot));
        }
        for field in 4..=u8::MAX {
            assert!(!handler.is_valid_slot(SlotCodec::encode(ApiAddress::Scalar, field, handle)));
        }
        for api in ApiAddress::iter().filter(|api| *api != ApiAddress::Scalar) {
            assert!(!handler.is_valid_slot(SlotCodec::encode(api, 0, handle)));
        }

        // Reject malformed namespace, version, and reserved bytes.
        for index in [0, 4, 6] {
            let mut bytes = keys.value.to_be_bytes::<32>();
            bytes[index] ^= 0xff;
            assert!(!handler.is_valid_slot(U256::from_be_bytes(bytes)));
        }
    }
    assert!(!handler.is_valid_slot(U256::ZERO));
    assert!(!handler.is_valid_slot(U256::MAX));
}

#[test]
fn invalid_scalar_keys_revert_before_state_access() {
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    for key in [
        B256::ZERO,
        B256::from(U256::from(5)),
        B256::from(SlotCodec::encode(ApiAddress::Scalar, 4, handle)),
        B256::from(SlotCodec::encode(ApiAddress::Scalar, 5, handle)),
        B256::from(SlotCodec::encode(ApiAddress::Scalar, 255, handle)),
        B256::from(SlotCodec::encode(ApiAddress::Container, 2, handle)),
    ] {
        let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
        let block = BlockEnv::default();
        let cfg: CfgEnv = CfgEnv::default();
        let tx = TxEnv::default();
        let mut recorder = ReadRecorder::new(0);
        {
            let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
            for data in [
                IScalar::initCall {
                    key,
                    min: U256::ZERO,
                    max: U256::from(100),
                }
                .abi_encode(),
                IScalar::addCall {
                    key,
                    delta: U256::from(1),
                }
                .abi_encode(),
                IScalar::subCall {
                    key,
                    delta: U256::from(1),
                }
                .abi_encode(),
                IScalar::getCall { key }.abi_encode(),
            ] {
                let input = CallInput {
                    caller: owner,
                    target: ApiAddress::Scalar.address(),
                    code_address: ApiAddress::Scalar.address(),
                    value: U256::ZERO,
                    input: &data,
                    gas_limit: 500_000,
                    scheme: CallScheme::Call,
                };
                let output = router
                    .dispatch(&input, &mut internals, &mut recorder)
                    .unwrap()
                    .unwrap();
                assert_eq!(output.status, ExitStatus::Revert);
                assert!(recorder.drain().is_empty());
            }
        }
        assert!(journal.evm_state().is_empty());
    }
}

#[test]
fn scalar_calls_preserve_all_24_instance_key_bytes() {
    let owner = Address::repeat_byte(0x11);
    let first = scalar_key(Address::with_last_byte(1));
    let mut second = first;
    second[8] = 1;
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    for (key, delta) in [(first, U256::from(11)), (second, U256::from(22))] {
        let init = IScalar::initCall {
            key,
            min: U256::ZERO,
            max: U256::from(100),
        };
        assert_eq!(
            invoke(&mut journal, owner, &init, CallScheme::Call, U256::ZERO).status,
            ExitStatus::Return
        );
        let add = IScalar::addCall { key, delta };
        assert_eq!(
            invoke(&mut journal, owner, &add, CallScheme::Call, U256::ZERO).status,
            ExitStatus::Return
        );
    }
    for (key, expected) in [(first, U256::from(11)), (second, U256::from(22))] {
        let output = invoke(
            &mut journal,
            owner,
            &IScalar::getCall { key },
            CallScheme::StaticCall,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Return);
        assert_eq!(U256::abi_decode_validate(&output.output).unwrap(), expected);
    }
    assert_eq!(journal.evm_state()[&owner].storage.len(), 8);
}

#[test]
fn only_get_records_the_value_and_delta_observation() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    let mut recorder = ReadRecorder::new(55);
    let get = IScalar::getCall {
        key: scalar_key(handle),
    }
    .abi_encode();
    for (index, (data, scheme)) in [
        (
            IScalar::initCall {
                key: scalar_key(handle),
                min: U256::ZERO,
                max: U256::from(100),
            }
            .abi_encode(),
            CallScheme::Call,
        ),
        (
            IScalar::addCall {
                key: scalar_key(handle),
                delta: U256::from(12),
            }
            .abi_encode(),
            CallScheme::Call,
        ),
        (
            IScalar::subCall {
                key: scalar_key(handle),
                delta: U256::from(5),
            }
            .abi_encode(),
            CallScheme::Call,
        ),
        (get.clone(), CallScheme::Call),
        (get, CallScheme::StaticCall),
    ]
    .into_iter()
    .enumerate()
    {
        let input = CallInput {
            caller: owner,
            target: ApiAddress::Scalar.address(),
            code_address: ApiAddress::Scalar.address(),
            value: U256::ZERO,
            input: &data,
            gas_limit: 500_000,
            scheme,
        };
        let output = router
            .dispatch(&input, &mut internals, &mut recorder)
            .unwrap()
            .unwrap();
        assert_eq!(output.status, ExitStatus::Return);
        if index < 3 {
            assert!(recorder.drain().is_empty());
        } else {
            assert_eq!(
                U256::abi_decode_validate(&output.output).unwrap(),
                U256::from(7)
            );
        }
    }
    let reads = recorder.drain();
    assert_eq!(reads.len(), 2);
    for slot in &[slots(handle).value, slots(handle).delta] {
        let read = reads
            .iter()
            .find(|read| read.key == Key::Storage(owner, *slot))
            .unwrap();
        assert_eq!(read.id, 55);
        assert_eq!(read.data.operations.reads.count(), 2);
    }
}

#[test]
fn native_reads_are_isolated_between_evms_and_transactions() {
    let owner = Address::repeat_byte(0x11);
    let first = Address::with_last_byte(1);
    let second = Address::with_last_byte(2);
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut left = factory.create_evm_with_inspector(
        EmptyDB::default(),
        EvmEnv::default(),
        ReadRecorder::new(7),
    );
    let mut right = factory.create_evm_with_inspector(
        EmptyDB::default(),
        EvmEnv::default(),
        ReadRecorder::new(8),
    );
    for (evm, handles) in [
        (&mut left, &[first][..]),
        (&mut right, &[first, second][..]),
    ] {
        for &handle in handles {
            assert!(
                evm.transact(ArcologyTx::from(TxEnv {
                    caller: owner,
                    kind: TxKind::Call(ApiAddress::Scalar.address()),
                    data: IScalar::getCall {
                        key: scalar_key(handle)
                    }
                    .abi_encode()
                    .into(),
                    gas_limit: 500_000,
                    gas_price: 0,
                    ..Default::default()
                }))
                .unwrap()
                .result
                .is_success()
            );
        }
    }
    for (evm, handle, id) in [(&mut left, first, 7), (&mut right, second, 8)] {
        let reads = evm.drain_reads();
        assert_eq!(reads.len(), 2);
        for slot in &[slots(handle).value, slots(handle).delta] {
            let read = reads
                .iter()
                .find(|read| read.key == Key::Storage(owner, *slot))
                .unwrap();
            assert_eq!(read.id, id);
            assert_eq!(read.data.id, id);
            assert_eq!(read.data.operations.reads.count(), 1);
        }
        assert!(evm.drain_reads().is_empty());
    }
    left.reset_reads(99);
    left.transact(ArcologyTx::from(TxEnv {
        caller: owner,
        kind: TxKind::Call(ApiAddress::Scalar.address()),
        data: IScalar::getCall {
            key: scalar_key(second),
        }
        .abi_encode()
        .into(),
        gas_limit: 500_000,
        gas_price: 0,
        ..Default::default()
    }))
    .unwrap();
    let reads = left.drain_reads();
    assert_eq!(reads.len(), 2);
    assert!(reads.iter().all(|read| read.id == 99));
    for slot in &[slots(second).value, slots(second).delta] {
        assert!(
            reads
                .iter()
                .any(|read| read.key == Key::Storage(owner, *slot))
        );
    }
    assert!(right.drain_reads().is_empty());
}

#[test]
fn native_and_opcode_reads_merge_even_when_the_contract_reverts() {
    let owner = Address::repeat_byte(0x22);
    let handle = Address::with_last_byte(1);
    let slot = slots(handle).value;
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    for exit in [0xf3, 0xfd] {
        // Read the value with SLOAD, then forward calldata to SCALAR.get using STATICCALL.
        let mut code = vec![0x7f];
        code.extend_from_slice(&slot.to_be_bytes::<32>());
        code.extend_from_slice(&[
            0x54, 0x50, 0x36, 0x60, 0, 0x60, 0, 0x37, 0x60, 0x20, 0x60, 0, 0x36, 0x60, 0, 0x60,
            0x85, 0x61, 0xff, 0xff, 0xfa, 0x50, 0x60, 0x20, 0x60, 0, exit,
        ]);
        let code = Bytecode::new_raw(code.into());
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            owner,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        db.insert_account_storage(owner, slots(handle).max, U256::from(100))
            .unwrap();
        db.insert_account_storage(owner, slot, U256::from(42))
            .unwrap();
        db.insert_account_storage(owner, slots(handle).delta, U256::from(5))
            .unwrap();
        let mut evm =
            factory.create_evm_with_inspector(db, EvmEnv::default(), ReadRecorder::new(33));
        let result = evm
            .transact(ArcologyTx::from(TxEnv {
                caller: Address::repeat_byte(0x11),
                kind: TxKind::Call(owner),
                data: IScalar::getCall {
                    key: scalar_key(handle),
                }
                .abi_encode()
                .into(),
                gas_limit: 500_000,
                gas_price: 0,
                ..Default::default()
            }))
            .unwrap();
        assert_eq!(result.result.is_success(), exit == 0xf3);
        assert_eq!(
            U256::abi_decode_validate(result.result.output().unwrap()).unwrap(),
            U256::from(47)
        );
        let account = &result.state[&owner];
        assert_eq!(
            account.storage[&slot].present_value(),
            U256::from(if exit == 0xf3 { 47 } else { 42 })
        );
        assert_eq!(
            account.storage[&slots(handle).delta].present_value(),
            U256::from(if exit == 0xf3 { 0 } else { 5 })
        );
        let reads = evm.drain_reads();
        assert!(reads.iter().all(|read| read.id == 33 && read.data.id == 33));
        let storage: Vec<_> = reads
            .iter()
            .filter(|read| matches!(read.key, Key::Storage(..)))
            .collect();
        assert_eq!(storage.len(), 2);
        let value_read = storage
            .iter()
            .find(|read| read.key == Key::Storage(owner, slot))
            .unwrap();
        assert_eq!(value_read.data.operations.reads.count(), 2);
        let delta_read = storage
            .iter()
            .find(|read| read.key == Key::Storage(owner, slots(handle).delta))
            .unwrap();
        assert_eq!(delta_read.data.operations.reads.count(), 1);
    }
}

#[test]
fn scalar_store_and_load_round_trip() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
    for (index, (min, max, value)) in [
        (U256::ZERO, U256::ZERO, U256::ZERO),
        (U256::from(10), U256::from(100), U256::from(50)),
        (U256::ZERO, U256::MAX, U256::MAX),
    ]
    .into_iter()
    .enumerate()
    {
        let handle = Address::with_last_byte(index as u8);
        let state = crdt::Scalar::<U256> {
            limits: (min, max),
            value,
            delta: None,
        };
        let keys = Slot::new(SlotCodec::encode(ApiAddress::Scalar, 2, handle));
        assert_eq!(keys, slots(handle));
        assert_eq!(keys.store(&mut internals, owner, &state).unwrap(), keys);
        assert_eq!(keys.load(&mut internals, owner).unwrap(), (state, keys));
    }
}

#[test]
fn scalar_load_sees_journal_changes_and_checkpoint_reverts() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let call = IScalar::initCall {
        key: scalar_key(Address::with_last_byte(1)),
        min: U256::ZERO,
        max: U256::from(100),
    };
    invoke(&mut journal, owner, &call, CallScheme::Call, U256::ZERO);
    let checkpoint = journal.checkpoint();
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    {
        let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
        internals
            .sstore(
                owner,
                slots(Address::from_word(call.key)).value,
                U256::from(50),
            )
            .unwrap();
        let (state, loaded_slots) = slots(Address::from_word(call.key))
            .load(&mut internals, owner)
            .unwrap();
        assert_eq!(loaded_slots, slots(Address::from_word(call.key)));
        assert_eq!(state.value, U256::from(50));
        assert_eq!(state.limits, (call.min, call.max));
        assert_eq!(state.delta, None);
    }
    journal.checkpoint_revert(checkpoint);
    let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
    let (state, _) = slots(Address::from_word(call.key))
        .load(&mut internals, owner)
        .unwrap();
    assert_eq!(state.value, U256::ZERO);
    assert_eq!(state.limits, (call.min, call.max));
}

#[test]
fn scalar_store_and_load_preserve_full_width_pending_deltas() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
    for (index, (value, delta)) in [
        (U256::ZERO, DeltaOp::Add(U256::MAX)),
        (U256::MAX, DeltaOp::Sub(U256::MAX)),
        (U256::from(1) << 255, DeltaOp::Sub(U256::from(1) << 255)),
        (U256::from(50), DeltaOp::Sub(U256::from(7))),
    ]
    .into_iter()
    .enumerate()
    {
        let handle = Address::with_last_byte(index as u8);
        let state = crdt::Scalar::<U256> {
            limits: (U256::ZERO, U256::MAX),
            value,
            delta: Some(delta),
        };
        let keys = slots(handle);
        assert_eq!(keys.store(&mut internals, owner, &state).unwrap(), keys);
        assert_eq!(keys.load(&mut internals, owner).unwrap(), (state, keys));
        let encoded_delta = match delta {
            DeltaOp::Add(magnitude) => magnitude,
            DeltaOp::Sub(magnitude) => U256::ZERO.wrapping_sub(magnitude),
        };
        assert_eq!(
            internals.sload(owner, keys.delta).unwrap().data,
            encoded_delta
        );
    }
}

#[test]
fn scalar_store_rejects_invalid_pending_arithmetic_before_writing() {
    for (value, delta) in [
        (U256::MAX, DeltaOp::Add(U256::from(1))),
        (U256::ZERO, DeltaOp::Sub(U256::from(1))),
    ] {
        let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
        let block = BlockEnv::default();
        let cfg: CfgEnv = CfgEnv::default();
        let tx = TxEnv::default();
        let state = crdt::Scalar::<U256> {
            value,
            delta: Some(delta),
            ..Default::default()
        };
        {
            let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
            assert!(
                slots(Address::ZERO)
                    .store(&mut internals, Address::ZERO, &state)
                    .is_err()
            );
        }
        assert!(journal.evm_state().is_empty());
    }
}

#[test]
fn initializes_four_fields_and_returns_the_supplied_key() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let call = IScalar::initCall {
        key: scalar_key(Address::repeat_byte(0x33)),
        min: U256::from(10),
        max: U256::from(100),
    };
    let output = invoke(&mut journal, owner, &call, CallScheme::Call, U256::ZERO);
    assert_eq!(output.status, ExitStatus::Return);
    assert_eq!(B256::abi_decode_validate(&output.output).unwrap(), call.key);
    assert!(journal.evm_state()[&owner].is_touched());
    assert_eq!(journal.evm_state()[&owner].storage.len(), 4);
    assert_eq!(
        stored_values(&mut journal, owner, slots(Address::from_word(call.key))),
        [call.min, call.max, U256::ZERO, U256::ZERO]
    );
    let keys = slots(Address::from_word(call.key));
    for (field, slot) in [keys.min, keys.max, keys.value, keys.delta]
        .into_iter()
        .enumerate()
    {
        let bytes = slot.to_be_bytes::<32>();
        assert_eq!(&bytes[..6], &[b'A', b'R', b'C', b'O', VERSION, 0x85]);
        assert_eq!(bytes[6], 0);
        assert_eq!(bytes[7], field as u8);
        assert_eq!(&bytes[8..12], &[0; 4]);
        assert_eq!(&bytes[12..], &call.key.as_slice()[12..]);
    }
}

#[test]
fn caller_and_handle_namespaces_are_independent() {
    let mut journal = Journal::new(EmptyDB::default());
    let alice = Address::repeat_byte(0x11);
    let bob = Address::repeat_byte(0x22);
    let first = Address::with_last_byte(1);
    let second = Address::with_last_byte(2);
    for (owner, handle, min) in [(alice, first, 10), (alice, second, 20), (bob, first, 30)] {
        let call = IScalar::initCall {
            key: scalar_key(handle),
            min: U256::from(min),
            max: U256::from(100),
        };
        assert_eq!(
            invoke(&mut journal, owner, &call, CallScheme::Call, U256::ZERO).status,
            ExitStatus::Return
        );
    }
    for (owner, handle, expected) in [(alice, first, 10), (alice, second, 20), (bob, first, 30)] {
        assert_eq!(
            journal.sload(owner, slots(handle).min).unwrap().data,
            U256::from(expected)
        );
    }
}

#[test]
fn invalid_initialization_leaves_the_journal_unchanged() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    for (min, max) in [(20, 10), (1, 0)] {
        let call = IScalar::initCall {
            key: scalar_key(Address::with_last_byte(1)),
            min: U256::from(min),
            max: U256::from(max),
        };
        assert_eq!(
            invoke(&mut journal, owner, &call, CallScheme::Call, U256::ZERO).status,
            ExitStatus::Revert
        );
    }
    let call = IScalar::initCall {
        key: scalar_key(Address::with_last_byte(1)),
        min: U256::ZERO,
        max: U256::from(100),
    };
    for (scheme, value) in [
        (CallScheme::StaticCall, U256::ZERO),
        (CallScheme::CallCodeOrDelegateCall, U256::ZERO),
        (CallScheme::Call, U256::from(1)),
    ] {
        assert_eq!(
            invoke(&mut journal, owner, &call, scheme, value).status,
            ExitStatus::Revert
        );
    }
    assert!(journal.evm_state().is_empty());
}

#[test]
fn initialization_reverts_with_the_journal_checkpoint() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let call = IScalar::initCall {
        key: scalar_key(Address::with_last_byte(1)),
        min: U256::from(10),
        max: U256::from(100),
    };
    let checkpoint = journal.checkpoint();
    invoke(&mut journal, owner, &call, CallScheme::Call, U256::ZERO);
    journal.checkpoint_revert(checkpoint);
    assert_eq!(
        stored_values(&mut journal, owner, slots(Address::from_word(call.key))),
        [U256::ZERO; 4]
    );
}

#[test]
fn add_and_sub_defer_deltas_until_get_materializes_each_update() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    let init = IScalar::initCall {
        key: scalar_key(handle),
        min: U256::from(10),
        max: U256::from(100),
    };
    invoke(&mut journal, owner, &init, CallScheme::Call, U256::ZERO);
    for (delta, expected) in [
        (DeltaOp::Add(U256::from(50)), 50),
        (DeltaOp::Sub(U256::from(20)), 30),
        (DeltaOp::Add(U256::from(70)), 100),
        (DeltaOp::Sub(U256::from(90)), 10),
        (DeltaOp::Add(U256::ZERO), 10),
        (DeltaOp::Sub(U256::ZERO), 10),
    ] {
        let output = invoke_delta(
            &mut journal,
            owner,
            handle,
            delta,
            CallScheme::Call,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Return);
        assert!(bool::abi_decode_validate(&output.output).unwrap());
        let value = U256::from(expected);
        let output = invoke(
            &mut journal,
            owner,
            &IScalar::getCall {
                key: scalar_key(handle),
            },
            CallScheme::StaticCall,
            U256::ZERO,
        );
        assert_eq!(U256::abi_decode_validate(&output.output).unwrap(), value);
        assert_eq!(
            stored_values(&mut journal, owner, slots(handle)),
            [init.min, init.max, value, U256::ZERO]
        );
    }
}

#[test]
fn add_accumulates_pending_deltas_and_get_resets_them() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    invoke(
        &mut journal,
        owner,
        &IScalar::initCall {
            key: scalar_key(handle),
            min: U256::ZERO,
            max: U256::from(100),
        },
        CallScheme::Call,
        U256::ZERO,
    );
    for (delta, expected) in [
        (DeltaOp::Add(U256::from(20)), 20),
        (DeltaOp::Add(U256::from(30)), 50),
        (DeltaOp::Sub(U256::from(5)), 45),
    ] {
        let output = invoke_delta(
            &mut journal,
            owner,
            handle,
            delta,
            CallScheme::Call,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Return);
        assert_eq!(
            journal.sload(owner, slots(handle).value).unwrap().data,
            U256::ZERO
        );
        assert_eq!(
            journal.sload(owner, slots(handle).delta).unwrap().data,
            U256::from(expected)
        );
    }
    // Bounds are checked against value plus all pending updates, without materializing.
    let output = invoke(
        &mut journal,
        owner,
        &IScalar::addCall {
            key: scalar_key(handle),
            delta: U256::from(60),
        },
        CallScheme::Call,
        U256::ZERO,
    );
    assert_eq!(output.status, ExitStatus::Revert);
    assert_eq!(
        journal.sload(owner, slots(handle).delta).unwrap().data,
        U256::from(45)
    );
    let output = invoke(
        &mut journal,
        owner,
        &IScalar::getCall {
            key: scalar_key(handle),
        },
        CallScheme::StaticCall,
        U256::ZERO,
    );
    assert_eq!(
        U256::abi_decode_validate(&output.output).unwrap(),
        U256::from(45)
    );
    assert_eq!(
        journal.sload(owner, slots(handle).value).unwrap().data,
        U256::from(45)
    );
    assert_eq!(
        journal.sload(owner, slots(handle).delta).unwrap().data,
        U256::ZERO
    );
    invoke(
        &mut journal,
        owner,
        &IScalar::subCall {
            key: scalar_key(handle),
            delta: U256::from(10),
        },
        CallScheme::Call,
        U256::ZERO,
    );
    assert_eq!(
        journal.sload(owner, slots(handle).value).unwrap().data,
        U256::from(45)
    );
    assert_eq!(
        journal.sload(owner, slots(handle).delta).unwrap().data,
        U256::ZERO.wrapping_sub(U256::from(10))
    );
    let output = invoke(
        &mut journal,
        owner,
        &IScalar::getCall {
            key: scalar_key(handle),
        },
        CallScheme::StaticCall,
        U256::ZERO,
    );
    assert_eq!(
        U256::abi_decode_validate(&output.output).unwrap(),
        U256::from(35)
    );
    assert_eq!(
        journal.sload(owner, slots(handle).delta).unwrap().data,
        U256::ZERO
    );
}

#[test]
fn accumulated_pending_deltas_span_the_full_uint256_range() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    invoke(
        &mut journal,
        owner,
        &IScalar::initCall {
            key: scalar_key(handle),
            min: U256::ZERO,
            max: U256::MAX,
        },
        CallScheme::Call,
        U256::ZERO,
    );
    for delta in [U256::MAX - U256::from(1), U256::from(1)] {
        let output = invoke(
            &mut journal,
            owner,
            &IScalar::addCall {
                key: scalar_key(handle),
                delta,
            },
            CallScheme::Call,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Return);
    }
    assert_eq!(
        journal.sload(owner, slots(handle).value).unwrap().data,
        U256::ZERO
    );
    assert_eq!(
        journal.sload(owner, slots(handle).delta).unwrap().data,
        U256::MAX
    );
    let output = invoke(
        &mut journal,
        owner,
        &IScalar::getCall {
            key: scalar_key(handle),
        },
        CallScheme::StaticCall,
        U256::ZERO,
    );
    assert_eq!(
        U256::abi_decode_validate(&output.output).unwrap(),
        U256::MAX
    );
    for delta in [U256::MAX - U256::from(1), U256::from(1)] {
        let output = invoke(
            &mut journal,
            owner,
            &IScalar::subCall {
                key: scalar_key(handle),
                delta,
            },
            CallScheme::Call,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Return);
    }
    assert_eq!(
        journal.sload(owner, slots(handle).value).unwrap().data,
        U256::MAX
    );
    assert_eq!(
        journal.sload(owner, slots(handle).delta).unwrap().data,
        U256::from(1)
    );
    let output = invoke(
        &mut journal,
        owner,
        &IScalar::getCall {
            key: scalar_key(handle),
        },
        CallScheme::StaticCall,
        U256::ZERO,
    );
    assert_eq!(
        U256::abi_decode_validate(&output.output).unwrap(),
        U256::ZERO
    );
    assert_eq!(
        journal.sload(owner, slots(handle).value).unwrap().data,
        U256::ZERO
    );
    assert_eq!(
        journal.sload(owner, slots(handle).delta).unwrap().data,
        U256::ZERO
    );
}

#[test]
fn invalid_deltas_leave_scalar_storage_unchanged() {
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    for (min, max, value, delta) in [
        (
            U256::from(10),
            U256::from(100),
            U256::from(50),
            DeltaOp::Add(U256::from(51)),
        ),
        (
            U256::from(10),
            U256::from(100),
            U256::from(50),
            DeltaOp::Sub(U256::from(41)),
        ),
        (
            U256::from(10),
            U256::from(100),
            U256::ZERO,
            DeltaOp::Add(U256::from(9)),
        ),
        (
            U256::ZERO,
            U256::MAX,
            U256::MAX,
            DeltaOp::Add(U256::from(1)),
        ),
        (
            U256::ZERO,
            U256::MAX,
            U256::ZERO,
            DeltaOp::Sub(U256::from(1)),
        ),
    ] {
        let mut journal = Journal::new(EmptyDB::default());
        invoke(
            &mut journal,
            owner,
            &IScalar::initCall {
                key: scalar_key(handle),
                min,
                max,
            },
            CallScheme::Call,
            U256::ZERO,
        );
        journal.sstore(owner, slots(handle).value, value).unwrap();
        let output = invoke_delta(
            &mut journal,
            owner,
            handle,
            delta,
            CallScheme::Call,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Revert);
        assert_eq!(
            stored_values(&mut journal, owner, slots(handle)),
            [min, max, value, U256::ZERO]
        );
    }
}

#[test]
fn add_and_sub_support_full_uint256_magnitudes() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    invoke(
        &mut journal,
        owner,
        &IScalar::initCall {
            key: scalar_key(handle),
            min: U256::ZERO,
            max: U256::MAX,
        },
        CallScheme::Call,
        U256::ZERO,
    );
    for (delta, expected) in [
        (DeltaOp::Add(U256::MAX), U256::MAX),
        (DeltaOp::Sub(U256::MAX), U256::ZERO),
    ] {
        let output = invoke_delta(
            &mut journal,
            owner,
            handle,
            delta,
            CallScheme::Call,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Return);
        assert!(bool::abi_decode_validate(&output.output).unwrap());
        let output = invoke(
            &mut journal,
            owner,
            &IScalar::getCall {
                key: scalar_key(handle),
            },
            CallScheme::StaticCall,
            U256::ZERO,
        );
        assert_eq!(U256::abi_decode_validate(&output.output).unwrap(), expected);
        assert_eq!(
            journal.sload(owner, slots(handle).value).unwrap().data,
            expected
        );
        assert_eq!(
            journal.sload(owner, slots(handle).delta).unwrap().data,
            U256::ZERO
        );
    }
}

#[test]
fn add_and_sub_reject_invalid_call_contexts_without_loading_state() {
    let owner = Address::repeat_byte(0x11);
    for (scheme, value) in [
        (CallScheme::StaticCall, U256::ZERO),
        (CallScheme::CallCodeOrDelegateCall, U256::ZERO),
        (CallScheme::Call, U256::from(1)),
    ] {
        for delta in [DeltaOp::Add(U256::from(1)), DeltaOp::Sub(U256::from(1))] {
            let mut journal = Journal::new(EmptyDB::default());
            let output = invoke_delta(
                &mut journal,
                owner,
                Address::with_last_byte(1),
                delta,
                scheme,
                value,
            );
            assert_eq!(output.status, ExitStatus::Revert);
            assert!(journal.evm_state().is_empty());
        }
    }
}

#[test]
fn add_keeps_callers_and_handles_independent() {
    let mut journal = Journal::new(EmptyDB::default());
    let alice = Address::repeat_byte(0x11);
    let bob = Address::repeat_byte(0x22);
    let first = Address::with_last_byte(1);
    let second = Address::with_last_byte(2);
    let cases = [(alice, first, 10), (alice, second, 20), (bob, first, 30)];
    for (owner, handle, delta) in cases {
        invoke(
            &mut journal,
            owner,
            &IScalar::initCall {
                key: scalar_key(handle),
                min: U256::ZERO,
                max: U256::from(100),
            },
            CallScheme::Call,
            U256::ZERO,
        );
        let output = invoke(
            &mut journal,
            owner,
            &IScalar::addCall {
                key: scalar_key(handle),
                delta: U256::from(delta),
            },
            CallScheme::Call,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Return);
    }
    for (owner, handle, expected) in cases {
        let output = invoke(
            &mut journal,
            owner,
            &IScalar::getCall {
                key: scalar_key(handle),
            },
            CallScheme::StaticCall,
            U256::ZERO,
        );
        assert_eq!(
            U256::abi_decode_validate(&output.output).unwrap(),
            U256::from(expected)
        );
    }
}

#[test]
fn add_reverts_with_the_journal_checkpoint() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    invoke(
        &mut journal,
        owner,
        &IScalar::initCall {
            key: scalar_key(handle),
            min: U256::ZERO,
            max: U256::from(100),
        },
        CallScheme::Call,
        U256::ZERO,
    );
    let checkpoint = journal.checkpoint();
    let output = invoke(
        &mut journal,
        owner,
        &IScalar::addCall {
            key: scalar_key(handle),
            delta: U256::from(50),
        },
        CallScheme::Call,
        U256::ZERO,
    );
    assert_eq!(output.status, ExitStatus::Return);
    invoke(
        &mut journal,
        owner,
        &IScalar::getCall {
            key: scalar_key(handle),
        },
        CallScheme::StaticCall,
        U256::ZERO,
    );
    assert_eq!(
        journal.sload(owner, slots(handle).value).unwrap().data,
        U256::from(50)
    );
    journal.checkpoint_revert(checkpoint);
    assert_eq!(
        stored_values(&mut journal, owner, slots(handle)),
        [U256::ZERO, U256::from(100), U256::ZERO, U256::ZERO]
    );
}

#[test]
fn evm_sub_returns_true_and_exports_only_the_pending_delta() {
    assert_eq!(
        IScalar::addCall::SELECTOR,
        alloy_primitives::keccak256("add(bytes32,uint256)")[..4]
    );
    assert_eq!(
        IScalar::subCall::SELECTOR,
        alloy_primitives::keccak256("sub(bytes32,uint256)")[..4]
    );
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(7);
    let mut db = InMemoryDB::default();
    db.insert_account_storage(owner, slots(handle).max, U256::from(100))
        .unwrap();
    db.insert_account_storage(owner, slots(handle).value, U256::from(42))
        .unwrap();
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm(db, EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: owner,
            kind: TxKind::Call(ApiAddress::Scalar.address()),
            data: IScalar::subCall {
                key: scalar_key(handle),
                delta: U256::from(2),
            }
            .abi_encode()
            .into(),
            gas_limit: 500_000,
            gas_price: 0,
            ..Default::default()
        }))
        .unwrap();
    assert!(result.result.is_success());
    assert!(bool::abi_decode_validate(result.result.output().unwrap()).unwrap());
    assert_eq!(
        result.state[&owner].storage[&slots(handle).value].present_value(),
        U256::from(42)
    );
    assert_eq!(
        result.state[&owner].storage[&slots(handle).delta].present_value(),
        U256::ZERO.wrapping_sub(U256::from(2))
    );
}

#[test]
fn get_returns_caller_scoped_journal_values_for_call_and_staticcall() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let alice = Address::repeat_byte(0x11);
    let bob = Address::repeat_byte(0x22);
    let first = Address::with_last_byte(1);
    let second = Address::with_last_byte(2);
    let values = [
        (alice, first, U256::from(50)),
        (alice, second, U256::MAX),
        (bob, first, U256::from(30)),
    ];
    {
        let block = BlockEnv::default();
        let cfg: CfgEnv = CfgEnv::default();
        let tx = TxEnv::default();
        let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
        for (owner, handle, value) in values {
            let state = crdt::Scalar::<U256> {
                value,
                ..Default::default()
            };
            slots(handle).store(&mut internals, owner, &state).unwrap();
        }
    }
    for (owner, handle, expected) in values {
        for scheme in [CallScheme::Call, CallScheme::StaticCall] {
            let output = invoke(
                &mut journal,
                owner,
                &IScalar::getCall {
                    key: scalar_key(handle),
                },
                scheme,
                U256::ZERO,
            );
            assert_eq!(output.status, ExitStatus::Return);
            assert_eq!(output.output.len(), 32);
            assert_eq!(U256::abi_decode_validate(&output.output).unwrap(), expected);
            assert_eq!(
                journal.sload(owner, slots(handle).value).unwrap().data,
                expected
            );
        }
    }
}

#[test]
fn get_writes_back_zero_for_an_uninitialized_scalar() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let output = invoke(
        &mut journal,
        owner,
        &IScalar::getCall {
            key: scalar_key(Address::with_last_byte(1)),
        },
        CallScheme::StaticCall,
        U256::ZERO,
    );
    assert_eq!(output.status, ExitStatus::Return);
    assert_eq!(
        U256::abi_decode_validate(&output.output).unwrap(),
        U256::ZERO
    );
    assert!(journal.evm_state()[&owner].is_touched());
    assert_eq!(journal.evm_state()[&owner].storage.len(), 4);
}

#[test]
fn get_rejects_invalid_call_contexts_without_loading_state() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let api = ApiAddress::Scalar.address();
    let data = IScalar::getCall {
        key: scalar_key(Address::with_last_byte(1)),
    }
    .abi_encode();
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    let mut router = CallRouter::new();
    router.register(api, ScalarHandler).unwrap();
    for (scheme, value, target) in [
        (CallScheme::CallCodeOrDelegateCall, U256::ZERO, api),
        (CallScheme::Call, U256::from(1), api),
        (CallScheme::StaticCall, U256::ZERO, owner),
    ] {
        let input = CallInput {
            caller: owner,
            target,
            code_address: api,
            value,
            input: &data,
            gas_limit: 500_000,
            scheme,
        };
        let output = {
            let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
            router
                .dispatch(&input, &mut internals, &mut ReadRecorder::new(0))
                .unwrap()
                .unwrap()
        };
        assert_eq!(output.status, ExitStatus::Revert);
        assert!(journal.evm_state().is_empty());
    }
}

#[test]
fn evm_get_returns_the_callers_stored_scalar_value() {
    assert_eq!(
        IScalar::getCall::SELECTOR,
        alloy_primitives::keccak256("get(bytes32)")[..4]
    );
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(7);
    let mut db = InMemoryDB::default();
    db.insert_account_storage(owner, slots(handle).max, U256::from(100))
        .unwrap();
    db.insert_account_storage(owner, slots(handle).value, U256::from(42))
        .unwrap();
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm(db, EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: owner,
            kind: TxKind::Call(ApiAddress::Scalar.address()),
            data: IScalar::getCall {
                key: scalar_key(handle),
            }
            .abi_encode()
            .into(),
            gas_limit: 500_000,
            gas_price: 0,
            ..Default::default()
        }))
        .unwrap();
    assert!(result.result.is_success());
    assert_eq!(
        U256::abi_decode_validate(result.result.output().unwrap()).unwrap(),
        U256::from(42)
    );
}

#[test]
fn evm_exports_the_fields_under_the_caller_account() {
    assert_eq!(
        IScalar::initCall::SELECTOR,
        alloy_primitives::keccak256("init(bytes32,uint256,uint256)")[..4]
    );
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Scalar.address(), ScalarHandler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let owner = Address::repeat_byte(0x11);
    let call = IScalar::initCall {
        key: scalar_key(Address::with_last_byte(7)),
        min: U256::ZERO,
        max: U256::from(100),
    };
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: owner,
            kind: TxKind::Call(ApiAddress::Scalar.address()),
            data: call.abi_encode().into(),
            gas_limit: 500_000,
            gas_price: 0,
            ..Default::default()
        }))
        .unwrap();
    assert!(result.result.is_success());
    assert_eq!(
        B256::abi_decode_validate(result.result.output().unwrap()).unwrap(),
        call.key
    );
    let keys = slots(Address::from_word(call.key));
    assert_eq!(
        [keys.min, keys.max, keys.value, keys.delta]
            .map(|key| result.state[&owner].storage[&key].present_value()),
        [call.min, call.max, U256::ZERO, U256::ZERO]
    );
}
