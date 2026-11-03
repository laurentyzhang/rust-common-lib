use super::{CallHandler, I256Handler, II256, IRuntime, IU256, RuntimeHandler, U256Handler};
use crate::crdt::{
    self, Scalar,
    state::{CrdtValue, DeltaOp, Status},
};
use crate::eu::state::ApiStore;
use crate::eu::{
    ApiAddress, ArcologyEvmFactory, ArcologyTx, CallInput, CallOutput, CallRouter, CallScheme,
    ExitStatus, JobOutcome, ReadRecorder, ScalarAdapter, ScalarKeyCodec, StatDbCommitter, StateDb,
    key::Key,
};
use crate::namespace::Namespace;
use crate::namespace::codec::VERSION;
use crate::planner::{
    Planner,
    meta::property::Property,
    scheduler::{ID, Job},
};
use crate::store::traits::Committer;
use alloy_evm::{
    Evm, EvmEnv, EvmFactory, EvmInternals,
    revm::{
        Journal,
        context::{BlockEnv, CfgEnv, JournalTr, TxEnv},
        database::{EmptyDB, InMemoryDB},
        state::{Account, AccountInfo, Bytecode},
    },
};
use alloy_primitives::{Address, B256, Bytes, I256, KECCAK256_EMPTY, TxKind, U256};
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
        is_deferred: false,
        caller: owner,
        target: ApiAddress::U256.address(),
        code_address: ApiAddress::U256.address(),
        value,
        input: &data,
        gas_limit: 500_000,
        scheme,
    };
    let mut internals = EvmInternals::new(journal, &block, &cfg, &tx);
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::U256.address(), U256Handler)
        .unwrap();
    router
        .dispatch(&input, &mut internals, &mut ReadRecorder::new(0))
        .unwrap()
        .unwrap()
}

fn scalar_key(handle: Address) -> B256 {
    ScalarKeyCodec::encode(Namespace::U256, 2, handle).into()
}

fn slots(handle: Address) -> ScalarAdapter {
    ScalarAdapter::new(ScalarKeyCodec::encode(Namespace::U256, 2, handle))
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
            &IU256::addCall {
                key: scalar_key(handle),
                delta,
            },
            scheme,
            value,
        ),
        DeltaOp::Sub(delta) => invoke(
            journal,
            owner,
            &IU256::subCall {
                key: scalar_key(handle),
                delta,
            },
            scheme,
            value,
        ),
    }
}

fn stored_values(journal: &mut Journal<EmptyDB>, owner: Address, keys: ScalarAdapter) -> [U256; 5] {
    [
        keys.min,
        keys.max,
        keys.value,
        keys.pos_delta,
        keys.neg_delta,
    ]
    .map(|key| journal.sload(owner, key).unwrap().data)
}

#[test]
fn scalar_handler_recognizes_slots_through_the_trait() {
    use strum::IntoEnumIterator;

    let handler: &dyn CallHandler = &U256Handler;
    for handle in [Address::ZERO, Address::repeat_byte(0xff)] {
        let keys = slots(handle);
        for slot in [
            keys.min,
            keys.max,
            keys.value,
            keys.pos_delta,
            keys.neg_delta,
        ] {
            assert!(handler.is_valid_slot(slot));
        }
        for field in 5..=u8::MAX {
            assert!(!handler.is_valid_slot(ScalarKeyCodec::encode(Namespace::U256, field, handle)));
        }
        for api in ApiAddress::iter().filter(|api| *api != ApiAddress::U256) {
            let mut bytes = ScalarKeyCodec::encode(Namespace::U256, 0, handle).to_be_bytes::<32>();
            bytes[5] = api as u8;
            assert!(!handler.is_valid_slot(U256::from_be_bytes(bytes)));
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
        .register(ApiAddress::U256.address(), U256Handler)
        .unwrap();
    let mut other_namespace =
        ScalarKeyCodec::encode(Namespace::U256, 2, handle).to_be_bytes::<32>();
    other_namespace[5] = ApiAddress::Set as u8;
    for key in [
        B256::ZERO,
        B256::from(U256::from(5)),
        B256::from(ScalarKeyCodec::encode(Namespace::U256, 6, handle)),
        B256::from(ScalarKeyCodec::encode(Namespace::U256, 5, handle)),
        B256::from(ScalarKeyCodec::encode(Namespace::U256, 255, handle)),
        B256::from(U256::from_be_bytes(other_namespace)),
    ] {
        let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
        let block = BlockEnv::default();
        let cfg: CfgEnv = CfgEnv::default();
        let tx = TxEnv::default();
        let mut recorder = ReadRecorder::new(0);
        {
            let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
            for data in [
                IU256::initCall {
                    key,
                    min: U256::ZERO,
                    max: U256::from(100),
                }
                .abi_encode(),
                IU256::addCall {
                    key,
                    delta: U256::from(1),
                }
                .abi_encode(),
                IU256::subCall {
                    key,
                    delta: U256::from(1),
                }
                .abi_encode(),
                IU256::getCall { key }.abi_encode(),
            ] {
                let input = CallInput {
                    is_deferred: false,
                    caller: owner,
                    target: ApiAddress::U256.address(),
                    code_address: ApiAddress::U256.address(),
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
        let init = IU256::initCall {
            key,
            min: U256::ZERO,
            max: U256::from(100),
        };
        assert_eq!(
            invoke(&mut journal, owner, &init, CallScheme::Call, U256::ZERO).status,
            ExitStatus::Return
        );
        let add = IU256::addCall { key, delta };
        assert_eq!(
            invoke(&mut journal, owner, &add, CallScheme::Call, U256::ZERO).status,
            ExitStatus::Return
        );
    }
    for (key, expected) in [(first, U256::from(11)), (second, U256::from(22))] {
        let output = invoke(
            &mut journal,
            owner,
            &IU256::getCall { key },
            CallScheme::StaticCall,
            U256::ZERO,
        );
        assert_eq!(output.status, ExitStatus::Return);
        assert_eq!(U256::abi_decode_validate(&output.output).unwrap(), expected);
    }
    assert_eq!(journal.evm_state()[&owner].storage.len(), 10);
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
        .register(ApiAddress::U256.address(), U256Handler)
        .unwrap();
    let mut recorder = ReadRecorder::new(55);
    let get = IU256::getCall {
        key: scalar_key(handle),
    }
    .abi_encode();
    for (index, (data, scheme)) in [
        (
            IU256::initCall {
                key: scalar_key(handle),
                min: U256::ZERO,
                max: U256::from(100),
            }
            .abi_encode(),
            CallScheme::Call,
        ),
        (
            IU256::addCall {
                key: scalar_key(handle),
                delta: U256::from(12),
            }
            .abi_encode(),
            CallScheme::Call,
        ),
        (
            IU256::subCall {
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
            is_deferred: false,
            caller: owner,
            target: ApiAddress::U256.address(),
            code_address: ApiAddress::U256.address(),
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
    assert_eq!(reads.len(), 3);
    for slot in &[
        slots(handle).value,
        slots(handle).pos_delta,
        slots(handle).neg_delta,
    ] {
        let read = reads
            .iter()
            .find(|read| read.key == Key::Storage(owner, *slot))
            .unwrap();
        assert_eq!(read.id.global, 55);
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
        .register(ApiAddress::U256.address(), U256Handler)
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
                    kind: TxKind::Call(ApiAddress::U256.address()),
                    data: IU256::getCall {
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
        assert_eq!(reads.len(), 3);
        for slot in &[
            slots(handle).value,
            slots(handle).pos_delta,
            slots(handle).neg_delta,
        ] {
            let read = reads
                .iter()
                .find(|read| read.key == Key::Storage(owner, *slot))
                .unwrap();
            assert_eq!(read.id.global, id);
            assert_eq!(read.data.id, id);
            assert_eq!(read.data.operations.reads.count(), 1);
        }
        assert!(evm.drain_reads().is_empty());
    }
    left.reset_reads(99);
    left.transact(ArcologyTx::from(TxEnv {
        caller: owner,
        kind: TxKind::Call(ApiAddress::U256.address()),
        data: IU256::getCall {
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
    assert_eq!(reads.len(), 3);
    assert!(reads.iter().all(|read| read.id.global == 99));
    for slot in &[
        slots(second).value,
        slots(second).pos_delta,
        slots(second).neg_delta,
    ] {
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
        .register(ApiAddress::U256.address(), U256Handler)
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
        db.insert_account_storage(owner, slots(handle).pos_delta, U256::from(5))
            .unwrap();
        let mut evm =
            factory.create_evm_with_inspector(db, EvmEnv::default(), ReadRecorder::new(33));
        let result = evm
            .transact(ArcologyTx::from(TxEnv {
                caller: Address::repeat_byte(0x11),
                kind: TxKind::Call(owner),
                data: IU256::getCall {
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
        assert_eq!(account.storage[&slot].present_value(), U256::from(42));
        assert_eq!(
            account.storage[&slots(handle).pos_delta].present_value(),
            U256::from(5)
        );
        let reads = evm.drain_reads();
        assert!(
            reads
                .iter()
                .all(|read| read.id.global == 33 && read.data.id == 33)
        );
        let storage: Vec<_> = reads
            .iter()
            .filter(|read| matches!(read.key, Key::Storage(..)))
            .collect();
        assert_eq!(storage.len(), 3);
        let value_read = storage
            .iter()
            .find(|read| read.key == Key::Storage(owner, slot))
            .unwrap();
        assert_eq!(value_read.data.operations.reads.count(), 2);
        let delta_read = storage
            .iter()
            .find(|read| read.key == Key::Storage(owner, slots(handle).pos_delta))
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
            min,
            max,
            value,
            delta: None,
        };
        let keys = ScalarAdapter::new(ScalarKeyCodec::encode(Namespace::U256, 2, handle));
        assert_eq!(keys, slots(handle));
        keys.store(&mut internals, owner, &state).unwrap();
        assert_eq!(
            keys.load::<U256, _>(&mut internals, owner).unwrap(),
            (state, keys)
        );
    }
}

#[test]
fn scalar_load_sees_journal_changes_and_checkpoint_reverts() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let call = IU256::initCall {
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
            .load::<U256, _>(&mut internals, owner)
            .unwrap();
        assert_eq!(loaded_slots, slots(Address::from_word(call.key)));
        assert_eq!(state.value, U256::from(50));
        assert_eq!((state.min, state.max), (call.min, call.max));
        assert_eq!(state.delta, None);
    }
    journal.checkpoint_revert(checkpoint);
    let mut internals = EvmInternals::new(&mut journal, &block, &cfg, &tx);
    let (state, _) = slots(Address::from_word(call.key))
        .load::<U256, _>(&mut internals, owner)
        .unwrap();
    assert_eq!(state.value, U256::ZERO);
    assert_eq!((state.min, state.max), (call.min, call.max));
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
            min: U256::ZERO,
            max: U256::MAX,
            value,
            delta: Some(delta),
        };
        let keys = slots(handle);
        keys.store(&mut internals, owner, &state).unwrap();
        assert_eq!(
            keys.load::<U256, _>(&mut internals, owner).unwrap(),
            (state, keys)
        );
        let expected = match delta {
            DeltaOp::Add(magnitude) => (magnitude, U256::ZERO),
            DeltaOp::Sub(magnitude) => (U256::ZERO, magnitude),
        };
        assert_eq!(
            (
                internals.read(owner, keys.pos_delta).unwrap(),
                internals.read(owner, keys.neg_delta).unwrap(),
            ),
            expected
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
                    .store(&mut internals, Address::ZERO, &state,)
                    .is_err()
            );
        }
        assert!(journal.evm_state().is_empty());
    }
}

#[test]
fn initializes_five_fields_and_returns_the_supplied_key() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let call = IU256::initCall {
        key: scalar_key(Address::repeat_byte(0x33)),
        min: U256::from(10),
        max: U256::from(100),
    };
    let output = invoke(&mut journal, owner, &call, CallScheme::Call, U256::ZERO);
    assert_eq!(output.status, ExitStatus::Return);
    assert_eq!(B256::abi_decode_validate(&output.output).unwrap(), call.key);
    assert!(journal.evm_state()[&owner].is_touched());
    assert_eq!(journal.evm_state()[&owner].storage.len(), 5);
    assert_eq!(
        stored_values(&mut journal, owner, slots(Address::from_word(call.key))),
        [call.min, call.max, U256::ZERO, U256::ZERO, U256::ZERO]
    );
    let keys = slots(Address::from_word(call.key));
    for (field, slot) in [
        keys.min,
        keys.max,
        keys.value,
        keys.pos_delta,
        keys.neg_delta,
    ]
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
        let call = IU256::initCall {
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
        let call = IU256::initCall {
            key: scalar_key(Address::with_last_byte(1)),
            min: U256::from(min),
            max: U256::from(max),
        };
        assert_eq!(
            invoke(&mut journal, owner, &call, CallScheme::Call, U256::ZERO).status,
            ExitStatus::Revert
        );
    }
    let call = IU256::initCall {
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
    let call = IU256::initCall {
        key: scalar_key(Address::with_last_byte(1)),
        min: U256::from(10),
        max: U256::from(100),
    };
    let checkpoint = journal.checkpoint();
    invoke(&mut journal, owner, &call, CallScheme::Call, U256::ZERO);
    journal.checkpoint_revert(checkpoint);
    assert_eq!(
        stored_values(&mut journal, owner, slots(Address::from_word(call.key))),
        [U256::ZERO; 5]
    );
}

#[test]
fn add_and_sub_keep_deltas_pending_across_get_calls() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    let init = IU256::initCall {
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
            &IU256::getCall {
                key: scalar_key(handle),
            },
            CallScheme::StaticCall,
            U256::ZERO,
        );
        assert_eq!(U256::abi_decode_validate(&output.output).unwrap(), value);
        assert_eq!(
            stored_values(&mut journal, owner, slots(handle)),
            [init.min, init.max, U256::ZERO, value, U256::ZERO]
        );
    }
}

#[test]
fn get_preserves_pending_deltas_for_later_updates() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let handle = Address::with_last_byte(1);
    invoke(
        &mut journal,
        owner,
        &IU256::initCall {
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
            journal.sload(owner, slots(handle).pos_delta).unwrap().data,
            U256::from(expected)
        );
    }
    // Bounds are checked against value plus all pending updates, without materializing.
    let output = invoke(
        &mut journal,
        owner,
        &IU256::addCall {
            key: scalar_key(handle),
            delta: U256::from(60),
        },
        CallScheme::Call,
        U256::ZERO,
    );
    assert_eq!(output.status, ExitStatus::Revert);
    assert_eq!(
        journal.sload(owner, slots(handle).pos_delta).unwrap().data,
        U256::from(45)
    );
    let output = invoke(
        &mut journal,
        owner,
        &IU256::getCall {
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
        U256::ZERO
    );
    assert_eq!(
        journal.sload(owner, slots(handle).pos_delta).unwrap().data,
        U256::from(45)
    );
    invoke(
        &mut journal,
        owner,
        &IU256::subCall {
            key: scalar_key(handle),
            delta: U256::from(10),
        },
        CallScheme::Call,
        U256::ZERO,
    );
    assert_eq!(
        journal.sload(owner, slots(handle).value).unwrap().data,
        U256::ZERO
    );
    assert_eq!(
        journal.sload(owner, slots(handle).neg_delta).unwrap().data,
        U256::ZERO
    );
    let output = invoke(
        &mut journal,
        owner,
        &IU256::getCall {
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
        journal.sload(owner, slots(handle).pos_delta).unwrap().data,
        U256::from(35)
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
        &IU256::initCall {
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
            &IU256::addCall {
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
        journal.sload(owner, slots(handle).pos_delta).unwrap().data,
        U256::MAX
    );
    let output = invoke(
        &mut journal,
        owner,
        &IU256::getCall {
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
            &IU256::subCall {
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
        journal.sload(owner, slots(handle).neg_delta).unwrap().data,
        U256::ZERO
    );
    let output = invoke(
        &mut journal,
        owner,
        &IU256::getCall {
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
        journal.sload(owner, slots(handle).pos_delta).unwrap().data,
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
            &IU256::initCall {
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
            [min, max, value, U256::ZERO, U256::ZERO]
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
        &IU256::initCall {
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
            &IU256::getCall {
                key: scalar_key(handle),
            },
            CallScheme::StaticCall,
            U256::ZERO,
        );
        assert_eq!(U256::abi_decode_validate(&output.output).unwrap(), expected);
        assert_eq!(
            journal.sload(owner, slots(handle).value).unwrap().data,
            U256::ZERO
        );
        assert_eq!(
            journal.sload(owner, slots(handle).pos_delta).unwrap().data,
            expected
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
            &IU256::initCall {
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
            &IU256::addCall {
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
            &IU256::getCall {
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
        &IU256::initCall {
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
        &IU256::addCall {
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
        &IU256::getCall {
            key: scalar_key(handle),
        },
        CallScheme::StaticCall,
        U256::ZERO,
    );
    assert_eq!(
        journal.sload(owner, slots(handle).value).unwrap().data,
        U256::ZERO
    );
    assert_eq!(
        journal.sload(owner, slots(handle).pos_delta).unwrap().data,
        U256::from(50)
    );
    journal.checkpoint_revert(checkpoint);
    assert_eq!(
        stored_values(&mut journal, owner, slots(handle)),
        [
            U256::ZERO,
            U256::from(100),
            U256::ZERO,
            U256::ZERO,
            U256::ZERO
        ]
    );
}

#[test]
fn evm_sub_returns_true_and_exports_only_the_pending_delta() {
    assert_eq!(
        IU256::addCall::SELECTOR,
        alloy_primitives::keccak256("add(bytes32,uint256)")[..4]
    );
    assert_eq!(
        IU256::subCall::SELECTOR,
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
        .register(ApiAddress::U256.address(), U256Handler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm(db, EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: owner,
            kind: TxKind::Call(ApiAddress::U256.address()),
            data: IU256::subCall {
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
        result.state[&owner].storage[&slots(handle).neg_delta].present_value(),
        U256::from(2)
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
                &IU256::getCall {
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
fn get_returns_zero_without_touching_an_uninitialized_scalar() {
    let mut journal = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let output = invoke(
        &mut journal,
        owner,
        &IU256::getCall {
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
    assert!(!journal.evm_state()[&owner].is_touched());
    assert_eq!(journal.evm_state()[&owner].storage.len(), 5);
}

#[test]
fn get_rejects_invalid_call_contexts_without_loading_state() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let owner = Address::repeat_byte(0x11);
    let api = ApiAddress::U256.address();
    let data = IU256::getCall {
        key: scalar_key(Address::with_last_byte(1)),
    }
    .abi_encode();
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    let mut router = CallRouter::new();
    router.register(api, U256Handler).unwrap();
    for (scheme, value, target) in [
        (CallScheme::CallCodeOrDelegateCall, U256::ZERO, api),
        (CallScheme::Call, U256::from(1), api),
        (CallScheme::StaticCall, U256::ZERO, owner),
    ] {
        let input = CallInput {
            is_deferred: false,
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
        IU256::getCall::SELECTOR,
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
        .register(ApiAddress::U256.address(), U256Handler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm(db, EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: owner,
            kind: TxKind::Call(ApiAddress::U256.address()),
            data: IU256::getCall {
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
        IU256::initCall::SELECTOR,
        alloy_primitives::keccak256("init(bytes32,uint256,uint256)")[..4]
    );
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::U256.address(), U256Handler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let owner = Address::repeat_byte(0x11);
    let call = IU256::initCall {
        key: scalar_key(Address::with_last_byte(7)),
        min: U256::ZERO,
        max: U256::from(100),
    };
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: owner,
            kind: TxKind::Call(ApiAddress::U256.address()),
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
        [
            keys.min,
            keys.max,
            keys.value,
            keys.pos_delta,
            keys.neg_delta
        ]
        .map(|key| result.state[&owner].storage[&key].present_value()),
        [call.min, call.max, U256::ZERO, U256::ZERO, U256::ZERO]
    );
}

fn invoke_i256(
    journal: &mut Journal<EmptyDB>,
    owner: Address,
    call: &impl SolCall,
    scheme: CallScheme,
) -> CallOutput {
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    let data = call.abi_encode();
    let input = CallInput {
        is_deferred: false,
        caller: owner,
        target: ApiAddress::I256.address(),
        code_address: ApiAddress::I256.address(),
        value: U256::ZERO,
        input: &data,
        gas_limit: 500_000,
        scheme,
    };
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::I256.address(), I256Handler)
        .unwrap();
    let mut internals = EvmInternals::new(journal, &block, &cfg, &tx);
    router
        .dispatch(&input, &mut internals, &mut ReadRecorder::new(0))
        .unwrap()
        .unwrap()
}

#[test]
fn signed_deltas_cross_zero_without_materializing() {
    let owner = Address::repeat_byte(0x11);
    let key: B256 = ScalarKeyCodec::encode(Namespace::I256, 2, Address::ZERO).into();
    let slots = ScalarAdapter::new(key.into());
    let mut journal = Journal::new(EmptyDB::default());
    assert_eq!(
        invoke_i256(
            &mut journal,
            owner,
            &II256::initCall {
                key,
                min: I256::unchecked_from(-100),
                max: I256::unchecked_from(100)
            },
            CallScheme::Call
        )
        .status,
        ExitStatus::Return
    );
    for (delta, expected) in [
        (DeltaOp::Sub(U256::from(30)), -30),
        (DeltaOp::Add(U256::from(50)), 20),
    ] {
        let output = match delta {
            DeltaOp::Add(delta) => invoke_i256(
                &mut journal,
                owner,
                &II256::addCall { key, delta },
                CallScheme::Call,
            ),
            DeltaOp::Sub(delta) => invoke_i256(
                &mut journal,
                owner,
                &II256::subCall { key, delta },
                CallScheme::Call,
            ),
        };
        assert_eq!(output.status, ExitStatus::Return);
        let output = invoke_i256(
            &mut journal,
            owner,
            &II256::getCall { key },
            CallScheme::StaticCall,
        );
        assert_eq!(
            I256::abi_decode_validate(&output.output).unwrap(),
            I256::unchecked_from(expected)
        );
        assert_eq!(journal.sload(owner, slots.value).unwrap().data, U256::ZERO);
    }
    assert_eq!(
        journal.sload(owner, slots.pos_delta).unwrap().data,
        U256::from(20)
    );
    assert_eq!(
        journal.sload(owner, slots.neg_delta).unwrap().data,
        U256::ZERO
    );
}

#[test]
fn signed_arithmetic_supports_full_width_magnitudes_and_rejects_overflow() {
    let owner = Address::repeat_byte(0x11);
    let key: B256 = ScalarKeyCodec::encode(Namespace::I256, 2, Address::ZERO).into();
    let slots = ScalarAdapter::new(key.into());
    let mut journal = Journal::new(EmptyDB::default());
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    slots
        .store(
            &mut EvmInternals::new(&mut journal, &block, &cfg, &tx),
            owner,
            &Scalar::<I256> {
                value: I256::MIN,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        invoke_i256(
            &mut journal,
            owner,
            &II256::addCall {
                key,
                delta: U256::MAX
            },
            CallScheme::Call
        )
        .status,
        ExitStatus::Return
    );
    let output = invoke_i256(
        &mut journal,
        owner,
        &II256::getCall { key },
        CallScheme::StaticCall,
    );
    assert_eq!(
        I256::abi_decode_validate(&output.output).unwrap(),
        I256::MAX
    );
    assert_eq!(
        invoke_i256(
            &mut journal,
            owner,
            &II256::addCall {
                key,
                delta: U256::from(1)
            },
            CallScheme::Call
        )
        .status,
        ExitStatus::Revert
    );
    assert_eq!(
        journal.sload(owner, slots.pos_delta).unwrap().data,
        U256::MAX
    );
    assert_eq!(
        invoke_i256(
            &mut journal,
            owner,
            &II256::subCall {
                key,
                delta: U256::MAX
            },
            CallScheme::Call
        )
        .status,
        ExitStatus::Return
    );
    assert_eq!(
        invoke_i256(
            &mut journal,
            owner,
            &II256::subCall {
                key,
                delta: U256::from(1)
            },
            CallScheme::Call
        )
        .status,
        ExitStatus::Revert
    );
    assert_eq!(
        journal.sload(owner, slots.value).unwrap().data,
        I256::MIN.into_raw()
    );
    assert_eq!(
        journal.sload(owner, slots.pos_delta).unwrap().data,
        U256::ZERO
    );
}

#[test]
fn signed_calls_reject_wrong_keys_bounds_and_call_modes() {
    let owner = Address::repeat_byte(0x11);
    let key: B256 = ScalarKeyCodec::encode(Namespace::I256, 2, Address::ZERO).into();
    for scheme in [CallScheme::StaticCall, CallScheme::CallCodeOrDelegateCall] {
        let mut journal = Journal::new(EmptyDB::default());
        assert_eq!(
            invoke_i256(
                &mut journal,
                owner,
                &II256::initCall {
                    key,
                    min: I256::MIN,
                    max: I256::MAX
                },
                scheme
            )
            .status,
            ExitStatus::Revert
        );
        assert!(journal.evm_state().is_empty());
    }
    let mut journal = Journal::new(EmptyDB::default());
    assert_eq!(
        invoke_i256(
            &mut journal,
            owner,
            &II256::initCall {
                key,
                min: I256::MAX,
                max: I256::MIN
            },
            CallScheme::Call
        )
        .status,
        ExitStatus::Revert
    );
    let unsigned: B256 = ScalarKeyCodec::encode(Namespace::U256, 2, Address::ZERO).into();
    assert_eq!(
        invoke_i256(
            &mut journal,
            owner,
            &II256::getCall { key: unsigned },
            CallScheme::Call
        )
        .status,
        ExitStatus::Revert
    );
    assert!(journal.evm_state().is_empty());
    assert!(!U256Handler.is_valid_slot(key.into()));
    assert!(!I256Handler.is_valid_slot(unsigned.into()));
    assert_eq!(II256::getCall::SELECTOR, IU256::getCall::SELECTOR);
}

#[test]
fn signed_evm_output_keeps_signed_numeric_records_and_rolls_back() {
    let owner = Address::repeat_byte(0x11);
    let key: B256 = ScalarKeyCodec::encode(Namespace::I256, 2, Address::ZERO).into();
    let slots = ScalarAdapter::new(key.into());
    let mut db = InMemoryDB::default();
    for (slot, value) in [
        (slots.min, I256::MIN.into_raw()),
        (slots.max, I256::MAX.into_raw()),
    ] {
        db.insert_account_storage(owner, slot, value).unwrap();
    }
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::I256.address(), I256Handler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm_with_inspector(db, EvmEnv::default(), ReadRecorder::new(7));
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: owner,
            kind: TxKind::Call(ApiAddress::I256.address()),
            data: II256::subCall {
                key,
                delta: U256::from(20),
            }
            .abi_encode()
            .into(),
            gas_limit: 500_000,
            gas_price: 0,
            ..Default::default()
        }))
        .unwrap();
    assert!(result.result.is_success());
    let outcome = JobOutcome::<(), _>::new(ID::new(7), Ok(result), None, evm.drain_reads());
    let records = outcome.merge_records(outcome.id);
    let record = &records
        .iter()
        .find(|(key, _)| *key == Key::Storage(owner, slots.value))
        .unwrap()
        .1;
    let Status::Value(CrdtValue::I256(scalar)) = record.current() else {
        panic!("expected a signed scalar");
    };
    assert_eq!(
        (scalar.min, scalar.max, scalar.value),
        (I256::MIN, I256::MAX, I256::ZERO)
    );
    assert_eq!(scalar.delta, Some(DeltaOp::Sub(U256::from(20))));

    let mut journal = Journal::new(EmptyDB::default());
    let checkpoint = journal.checkpoint();
    invoke_i256(
        &mut journal,
        owner,
        &II256::initCall {
            key,
            min: I256::MIN,
            max: I256::MAX,
        },
        CallScheme::Call,
    );
    invoke_i256(
        &mut journal,
        owner,
        &II256::subCall {
            key,
            delta: U256::from(20),
        },
        CallScheme::Call,
    );
    journal.checkpoint_revert(checkpoint);
    assert_eq!(
        journal.sload(owner, slots.neg_delta).unwrap().data,
        U256::ZERO
    );
}

fn factory() -> ArcologyEvmFactory {
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), RuntimeHandler)
        .unwrap();
    ArcologyEvmFactory::try_new(router).unwrap()
}

fn deferral_slot(host: Address, selector: [u8; 4]) -> U256 {
    let mut callee = [0; 8];
    callee[..4].copy_from_slice(&host[..4]);
    callee[4..].copy_from_slice(&selector);
    Property::DeferredGas.slot(host, u64::from_le_bytes(callee), 0)
}

fn invoke_runtime(
    journal: &mut Journal<EmptyDB>,
    owner: Address,
    data: &[u8],
    scheme: CallScheme,
) -> CallOutput {
    let input = CallInput {
        caller: owner,
        target: ApiAddress::Runtime.address(),
        code_address: ApiAddress::Runtime.address(),
        value: U256::ZERO,
        input: data,
        gas_limit: 500_000,
        scheme,
        is_deferred: false,
    };
    let block = BlockEnv::default();
    let cfg: CfgEnv = CfgEnv::default();
    let tx = TxEnv::default();
    factory()
        .router()
        .dispatch(
            &input,
            &mut EvmInternals::new(journal, &block, &cfg, &tx),
            &mut ReadRecorder::new(0),
        )
        .unwrap()
        .unwrap()
}

fn constructor(selector: [u8; 4], gas: u64, revert: bool) -> Bytes {
    let data = IRuntime::setDeferralCall {
        selector: selector.into(),
        prepaidGas: gas,
    }
    .abi_encode();
    // Copy the configuration call into memory, invoke_runtime Runtime, then finish deployment.
    let mut code = vec![
        0x60,
        data.len() as u8,
        0x61,
        0,
        0,
        0x60,
        0,
        0x39,
        0x60,
        0,
        0x60,
        0,
        0x60,
        data.len() as u8,
        0x60,
        0,
        0x60,
        0,
        0x60,
        0xa0,
        0x62,
        0x07,
        0xa1,
        0x20,
        0xf1,
        0x50,
    ];
    if revert {
        code.extend_from_slice(&[0x60, 0, 0x60, 0, 0xfd]);
    } else {
        code.extend_from_slice(&[0x60, 0, 0x60, 0, 0x53, 0x60, 1, 0x60, 0, 0xf3]);
    }
    let offset = code.len() as u16;
    code[3..5].copy_from_slice(&offset.to_be_bytes());
    code.extend(data);
    code.into()
}

#[test]
fn constructor_configuration_persists_and_drives_scheduling() {
    let selector = [0x12, 0x34, 0x56, 0x78];
    let factory = factory();
    let mut evm = factory.create_evm(InMemoryDB::default(), EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: Address::repeat_byte(0x11),
            kind: TxKind::Create,
            data: constructor(selector, 123_456, false),
            gas_limit: 1_000_000,
            gas_price: 0,
            ..Default::default()
        }))
        .unwrap();
    assert!(result.result.is_success());
    let host = result.result.created_address().unwrap();
    let slot = deferral_slot(host, selector);
    assert_eq!(
        result.state[&host].storage[&slot].present_value(),
        U256::from(123_456)
    );
    assert_ne!(result.state[&host].info.code_hash, KECCAK256_EMPTY);
    let id = ID::new(7);
    let mut rejected = StatDbCommitter::new();
    rejected.buffer(id, result.state.clone());
    rejected.stage(Vec::new());
    rejected.flush();
    assert!(rejected.inner().is_empty());

    let mut accepted = StatDbCommitter::new();
    accepted.buffer(id, result.state);
    accepted.stage(vec![id]);
    accepted.flush();
    let db = StateDb::new(accepted.into_inner());
    let mut planner = Planner::<_, u64>::new(db, Default::default());
    for (gas, expected) in [(123_456, true), (0, false)] {
        planner.db.write(host, slot, U256::from(gas)).unwrap();
        let scheduled = planner
            .plan([Job::new(
                ID::new(7),
                (
                    TxEnv {
                        caller: Address::repeat_byte(0x22),
                        kind: TxKind::Call(host),
                        data: selector.to_vec().into(),
                        ..Default::default()
                    },
                    (),
                ),
            )])
            .unwrap();
        assert_eq!(scheduled[0].sequences[0].jobs[0].is_deferred, expected);
    }
}

#[test]
fn configuration_is_owned_by_the_child_in_create_and_create2() {
    let parent = Address::repeat_byte(0x33);
    let selector = [1, 2, 3, 4];
    for create2 in [false, true] {
        let child = constructor(selector, u64::MAX, false);
        let mut code = vec![0x61, 0, 0, 0x61, 0, 0, 0x60, 0, 0x39];
        code[1..3].copy_from_slice(&(child.len() as u16).to_be_bytes());
        if create2 {
            code.extend_from_slice(&[0x60, 0x42]);
        }
        code.push(0x61);
        code.extend_from_slice(&(child.len() as u16).to_be_bytes());
        code.extend_from_slice(&[
            0x60,
            0,
            0x60,
            0,
            if create2 { 0xf5 } else { 0xf0 },
            0x60,
            0,
            0x52,
            0x60,
            32,
            0x60,
            0,
            0xf3,
        ]);
        let offset = code.len() as u16;
        code[4..6].copy_from_slice(&offset.to_be_bytes());
        code.extend_from_slice(&child);
        let code = Bytecode::new_raw(code.into());
        let mut db = InMemoryDB::default();
        db.insert_account_info(
            parent,
            AccountInfo {
                code_hash: code.hash_slow(),
                code: Some(code),
                ..Default::default()
            },
        );
        let factory = factory();
        let mut evm = factory.create_evm(db, EvmEnv::default());
        let result = evm
            .transact(ArcologyTx::from(TxEnv {
                caller: Address::repeat_byte(0x11),
                kind: TxKind::Call(parent),
                gas_limit: 1_000_000,
                gas_price: 0,
                ..Default::default()
            }))
            .unwrap();
        assert!(result.result.is_success());
        let child = Address::from_word(B256::from_slice(result.result.output().unwrap()));
        assert_ne!(child, Address::ZERO);
        assert_eq!(
            result.state[&child].storage[&deferral_slot(child, selector)].present_value(),
            U256::from(u64::MAX)
        );
        assert!(
            !result.state[&parent]
                .storage
                .contains_key(&deferral_slot(parent, selector))
        );
    }
}

#[test]
fn reverting_deployment_removes_deferral_configuration() {
    let factory = factory();
    let mut evm = factory.create_evm(InMemoryDB::default(), EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: Address::repeat_byte(0x11),
            kind: TxKind::Create,
            data: constructor([1, 2, 3, 4], 123, true),
            gas_limit: 1_000_000,
            gas_price: 0,
            ..Default::default()
        }))
        .unwrap();
    assert!(!result.result.is_success());
    assert!(result.state.values().all(|account| {
        account
            .storage
            .values()
            .all(|slot| slot.present_value().is_zero() && !slot.is_changed())
    }));
}

#[test]
fn registration_rejects_eoas_deployed_code_and_indirect_or_static_calls() {
    let owner = Address::repeat_byte(0x11);
    let data = IRuntime::setDeferralCall {
        selector: [1, 2, 3, 4].into(),
        prepaidGas: 123,
    }
    .abi_encode();
    for (local, deployed, scheme) in [
        (false, false, CallScheme::Call),
        (true, true, CallScheme::Call),
        (true, false, CallScheme::StaticCall),
        (true, false, CallScheme::CallCodeOrDelegateCall),
    ] {
        let mut journal = Journal::new(EmptyDB::default());
        let mut account = Account::default().with_created_mark();
        if local {
            account.mark_created_locally();
        }
        if deployed {
            let code = Bytecode::new_raw(Bytes::from_static(&[0]));
            account.info.code_hash = code.hash_slow();
            account.info.code = Some(code);
        }
        journal.evm_state_mut().insert(owner, account);
        assert_eq!(
            invoke_runtime(&mut journal, owner, &data, scheme).status,
            ExitStatus::Revert
        );
        assert!(journal.evm_state()[&owner].storage.is_empty());
    }
}

#[test]
fn function_settings_are_independent_and_malformed_calls_do_not_write() {
    let owner = Address::repeat_byte(0x11);
    let mut journal = Journal::new(EmptyDB::default());
    let mut account = Account::default();
    account.mark_created_locally();
    journal.evm_state_mut().insert(owner, account);
    for (selector, gas) in [([1, 2, 3, 4], 123), ([5, 6, 7, 8], 456), ([1, 2, 3, 4], 0)] {
        let data = IRuntime::setDeferralCall {
            selector: selector.into(),
            prepaidGas: gas,
        }
        .abi_encode();
        assert_eq!(
            invoke_runtime(&mut journal, owner, &data, CallScheme::Call).status,
            ExitStatus::Return
        );
    }
    assert_eq!(
        journal.evm_state()[&owner].storage[&deferral_slot(owner, [1, 2, 3, 4])].present_value(),
        U256::ZERO
    );
    assert_eq!(
        journal.evm_state()[&owner].storage[&deferral_slot(owner, [5, 6, 7, 8])].present_value(),
        U256::from(456)
    );
    for data in [
        vec![],
        vec![0xff; 4],
        IRuntime::setDeferralCall::SELECTOR.to_vec(),
    ] {
        assert_eq!(
            invoke_runtime(&mut journal, owner, &data, CallScheme::Call).status,
            ExitStatus::Revert
        );
    }
    assert_eq!(journal.evm_state()[&owner].storage.len(), 2);
}

#[test]
fn execution_phase_is_evm_local_and_cleared_after_each_transaction() {
    let owner = Address::repeat_byte(0x11);
    let nested = Address::repeat_byte(0x22);
    let code = Bytecode::new_raw(Bytes::from_static(&[
        0x36, 0x60, 0, 0x60, 0, 0x37, 0x60, 32, 0x60, 0, 0x36, 0x60, 0, 0x60, 0xa0, 0x61, 0xff,
        0xff, 0xfa, 0x50, 0x60, 32, 0x60, 0, 0xf3,
    ]));
    let mut db = InMemoryDB::default();
    db.insert_account_info(
        nested,
        AccountInfo {
            code_hash: code.hash_slow(),
            code: Some(code),
            ..Default::default()
        },
    );
    let factory = factory();
    let mut left = factory.create_evm(db, EvmEnv::default());
    let mut right = factory.create_evm(InMemoryDB::default(), EvmEnv::default());
    left.set_deferred(true);
    for (use_left, target, expected) in [
        (false, ApiAddress::Runtime.address(), false),
        (true, nested, true),
        (true, ApiAddress::Runtime.address(), false),
    ] {
        let evm = if use_left { &mut left } else { &mut right };
        let result = evm
            .transact(ArcologyTx::from(TxEnv {
                caller: owner,
                kind: TxKind::Call(target),
                data: IRuntime::isDeferredCall {}.abi_encode().into(),
                gas_limit: 500_000,
                gas_price: 0,
                ..Default::default()
            }))
            .unwrap();
        assert!(result.result.is_success());
        assert_eq!(
            bool::abi_decode_validate(result.result.output().unwrap()).unwrap(),
            expected
        );
    }
}
