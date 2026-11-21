use super::{SetAdapter, SetKeyCodec, SetProperty};
use crate::execution::evm::storage::ApiStore;
use crate::execution::{
    ApiAddress, CallInput, CallOutput, CallRouter, CallScheme, ISet, Recorder, SetHandler, StateDb,
};
use alloy_evm::{
    EvmInternals,
    revm::{
        Journal,
        context::{BlockEnv, CfgEnv, JournalTr, TxEnv},
        database::EmptyDB,
    },
};
use alloy_primitives::{Address, Bytes, U256};
use alloy_sol_types::{SolCall, SolValue};

const OWNER: Address = Address::repeat_byte(0x11);

/// Writes an element and one pending flag, as the handler does.
fn flag(db: &mut StateDb, element: &[u8], property: SetProperty) {
    let id = SetKeyCodec::element_id(element);
    SetAdapter::write_element(db, OWNER, id, element).unwrap();
    SetAdapter::flag(db, OWNER, id, property).unwrap();
}

/// Returns committed elements by index, with `None` for removed slots.
fn elements(db: &mut StateDb) -> Vec<Option<Vec<u8>>> {
    let length = db
        .read(OWNER, SetKeyCodec::encode(SetProperty::Length, [0; 24]))
        .unwrap()
        .to::<u64>();
    (0..length)
        .map(|i| {
            let word = db.read(OWNER, SetKeyCodec::index(i)).unwrap();
            (!word.is_zero()).then(|| {
                let id = word.to_be_bytes::<32>()[8..].try_into().unwrap();
                SetAdapter::read_element(db, OWNER, id).unwrap()
            })
        })
        .collect()
}

/// Calls the Set handler as OWNER through the router.
fn invoke(journal: &mut Journal<EmptyDB>, call: &impl SolCall, scheme: CallScheme) -> CallOutput {
    let (block, tx) = (BlockEnv::default(), TxEnv::default());
    let cfg: CfgEnv = CfgEnv::default();
    let data = call.abi_encode();
    let input = CallInput {
        is_deferred: false,
        caller: OWNER,
        target: ApiAddress::Set.address(),
        code_address: ApiAddress::Set.address(),
        value: U256::ZERO,
        input: &data,
        gas_limit: 500_000,
        scheme,
    };
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Set.address(), SetHandler)
        .unwrap();
    let mut internals = EvmInternals::new(journal, &block, &cfg, &tx);
    router
        .dispatch(&input, &mut internals, &mut Recorder::new(0))
        .unwrap()
        .unwrap()
}

#[test]
fn key_codec_round_trips_and_rejects_undefined_properties() {
    let id = SetKeyCodec::element_id(b"a");
    let key = SetKeyCodec::encode(SetProperty::Added, id);
    assert_eq!(SetKeyCodec::decode(key), Some((SetProperty::Added, id)));

    let mut bytes = key.to_be_bytes::<32>();
    bytes[7] = 9;
    assert_eq!(SetKeyCodec::decode(U256::from_be_bytes(bytes)), None);
}

#[test]
fn fold_appends_leaves_holes_and_resolves_add_wins() {
    let long = vec![0xab; 70];
    let mut db = StateDb::default();

    flag(&mut db, b"a", SetProperty::Added);
    SetAdapter::fold_pending(&mut db, OWNER).unwrap();
    flag(&mut db, b"b", SetProperty::Added);
    SetAdapter::fold_pending(&mut db, OWNER).unwrap();
    assert_eq!(
        elements(&mut db),
        [Some(b"a".to_vec()), Some(b"b".to_vec())]
    );

    // A removal leaves a hole; a multi-chunk element round-trips.
    flag(&mut db, b"a", SetProperty::Removed);
    flag(&mut db, &long, SetProperty::Added);
    SetAdapter::fold_pending(&mut db, OWNER).unwrap();
    assert_eq!(
        elements(&mut db),
        [None, Some(b"b".to_vec()), Some(long.clone())]
    );

    // Add and remove of the same element in one fold: removed, then re-added at a new index.
    flag(&mut db, b"b", SetProperty::Removed);
    flag(&mut db, b"b", SetProperty::Added);
    SetAdapter::fold_pending(&mut db, OWNER).unwrap();
    assert_eq!(
        elements(&mut db),
        [None, None, Some(long), Some(b"b".to_vec())]
    );

    let b = SetKeyCodec::element_id(b"b");
    assert_eq!(
        db.read(OWNER, SetKeyCodec::encode(SetProperty::Member, b))
            .unwrap(),
        U256::from(4)
    );
    for property in [SetProperty::Added, SetProperty::Removed] {
        assert!(
            db.read(OWNER, SetKeyCodec::encode(property, b))
                .unwrap()
                .is_zero()
        );
    }
}

/// Calls the handler and decodes the given return type.
fn call<T: SolValue + From<<T::SolType as alloy_sol_types::SolType>::RustType>>(
    journal: &mut Journal<EmptyDB>,
    call: &impl SolCall,
) -> T {
    T::abi_decode(&invoke(journal, call, CallScheme::Call).output).unwrap()
}

/// Returns the element at `index` through the handler.
fn at(journal: &mut Journal<EmptyDB>, index: u64) -> (bool, Bytes) {
    let output = invoke(
        journal,
        &ISet::atCall {
            index: U256::from(index),
        },
        CallScheme::StaticCall,
    );
    <(bool, Bytes)>::abi_decode_params(&output.output).unwrap()
}

#[test]
fn handler_assigns_indexes_on_read_in_insertion_order() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let [b, a] = [b"b", b"a"].map(|element| Bytes::from_static(element));

    let insert = ISet::insertCall { element: b.clone() };
    assert!(
        invoke(&mut journal, &insert, CallScheme::StaticCall)
            .output
            .is_empty()
    );
    assert!(call::<bool>(&mut journal, &insert));
    assert!(call::<bool>(
        &mut journal,
        &ISet::insertCall { element: a.clone() }
    ));

    // Reading by index folds pending inserts in insertion order.
    assert_eq!(
        call::<U256>(&mut journal, &ISet::lengthCall {}),
        U256::from(2)
    );
    assert_eq!(at(&mut journal, 0), (true, b.clone()));
    assert_eq!(at(&mut journal, 1), (true, a.clone()));
    assert_eq!(at(&mut journal, 2), (false, Bytes::new()));

    // A pending remove hides the element; the next index read leaves a hole.
    assert!(call::<bool>(
        &mut journal,
        &ISet::removeCall { element: b.clone() }
    ));
    assert!(!call::<bool>(
        &mut journal,
        &ISet::containsCall { element: b }
    ));
    assert_eq!(at(&mut journal, 0), (false, Bytes::new()));
    assert_eq!(at(&mut journal, 1), (true, a));
}
