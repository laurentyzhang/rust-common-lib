use super::{SetAdapter, SetKeyCodec, SetProperty};
use crate::execution::evm::storage::ApiStore;
use crate::execution::{
    ApiAddress, CallInput, CallOutput, CallRouter, CallScheme, ISet, Recorder, SetHandler,
};
use alloy_evm::{
    EvmInternals,
    revm::{
        Journal,
        context::{BlockEnv, CfgEnv, JournalTr, TxEnv},
        database::EmptyDB,
    },
};
use alloy_primitives::{Address, B256, U256, keccak256};
use alloy_sol_types::{SolCall, SolValue};

const OWNER: Address = Address::repeat_byte(0x11);

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

/// Calls the handler and decodes the given return type.
fn call<T: SolValue + From<<T::SolType as alloy_sol_types::SolType>::RustType>>(
    journal: &mut Journal<EmptyDB>,
    call: &impl SolCall,
) -> T {
    T::abi_decode(&invoke(journal, call, CallScheme::Call).output).unwrap()
}

/// Runs `f` against OWNER's storage through a fresh `EvmInternals` over `journal`.
fn with_internals<R>(journal: &mut Journal<EmptyDB>, f: impl FnOnce(&mut EvmInternals<'_>) -> R) -> R {
    let (block, tx) = (BlockEnv::default(), TxEnv::default());
    let cfg: CfgEnv = CfgEnv::default();
    f(&mut EvmInternals::new(journal, &block, &cfg, &tx))
}

/// Seeds `set` with committed `elements` as a circular list: Length, Tail and Next links.
fn commit(journal: &mut Journal<EmptyDB>, set: B256, elements: &[u64]) {
    let id = SetKeyCodec::derive_key(set.into());
    with_internals(journal, |internals| {
        let write = |internals: &mut EvmInternals<'_>, key, value| internals.write(OWNER, key, value).unwrap();
        write(internals, SetKeyCodec::length(id), U256::from(elements.len()));
        write(internals, SetKeyCodec::tail(id), U256::from(*elements.last().unwrap()));
        for (i, &element) in elements.iter().enumerate() {
            let next = U256::from(elements[(i + 1) % elements.len()]);
            // A successor of 0 is marked under flag 1.
            let (flag, value) = if next.is_zero() { (1, U256::from(1)) } else { (0, next) };
            write(internals, SetKeyCodec::element(SetProperty::Next, id, U256::from(element), flag), value);
        }
    });
}

/// Calls insert or remove on `set` and returns whether the pending delta changed.
fn stage(journal: &mut Journal<EmptyDB>, set: B256, element: u64, insert: bool) -> bool {
    let element = U256::from(element);
    if insert {
        call::<bool>(journal, &ISet::insertCall { set, element })
    } else {
        call::<bool>(journal, &ISet::removeCall { set, element })
    }
}

/// Returns whether `set` contains `element` through the handler.
fn contains(journal: &mut Journal<EmptyDB>, set: B256, element: u64) -> bool {
    call::<bool>(
        journal,
        &ISet::containsCall {
            set,
            element: U256::from(element),
        },
    )
}

/// Returns the live count of `set` through the handler.
fn length(journal: &mut Journal<EmptyDB>, set: B256) -> U256 {
    call::<U256>(journal, &ISet::lengthCall { set })
}

/// Traverses `set` with first/next through the handler.
fn elements(journal: &mut Journal<EmptyDB>, set: B256) -> Vec<u64> {
    let decode = |output: CallOutput| <(bool, U256)>::abi_decode_params(&output.output).unwrap();
    let mut found = Vec::new();
    let mut cursor = decode(invoke(journal, &ISet::firstCall { set }, CallScheme::StaticCall));
    while cursor.0 {
        found.push(cursor.1.to::<u64>());
        let call = ISet::nextCall { set, element: cursor.1 };
        cursor = decode(invoke(journal, &call, CallScheme::StaticCall));
    }
    found
}

#[test]
fn key_layout_groups_slots_by_set() {
    let set = SetKeyCodec::derive_key(U256::from(1));
    let element = U256::from(7);
    let key = SetKeyCodec::element(SetProperty::Next, set, element, 1).to_be_bytes::<32>();

    // [prefix:7][set ID:6][property:1][element hash:16][flag:1][reserved:1].
    assert_eq!(&key[..4], b"ARCO");
    assert_eq!(&key[7..13], &set);
    assert_eq!(key[13], SetProperty::Next as u8);
    assert_eq!(&key[14..30], &keccak256(element.to_be_bytes::<32>())[..16]);
    assert_eq!((key[30], key[31]), (1, 0));
    assert_eq!(
        SetKeyCodec::decode(U256::from_be_bytes(key)),
        Some((set, SetProperty::Next, key[14..30].try_into().unwrap(), 1))
    );

    // Log keys carry the index in the 16-byte field.
    let log = SetKeyCodec::delta(SetProperty::DeltaAdded, set).key(3).unwrap().to_be_bytes::<32>();
    assert_eq!(&log[7..13], &set);
    assert_eq!(log[13], SetProperty::DeltaAdded as u8);
    assert_eq!(&log[14..30], &3u128.to_be_bytes());
    assert_eq!((log[30], log[31]), (0, 0));

    // Undefined properties and a non-zero reserved byte are rejected.
    let mut bad = key;
    bad[13] = 8;
    assert_eq!(SetKeyCodec::decode(U256::from_be_bytes(bad)), None);
    let mut bad = key;
    bad[31] = 1;
    assert_eq!(SetKeyCodec::decode(U256::from_be_bytes(bad)), None);
    assert_ne!(SetKeyCodec::length(set), SetKeyCodec::length(SetKeyCodec::derive_key(U256::from(2))));
}

#[test]
fn handler_flags_pending_elements_per_set_and_reads_committed_indexes() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let (set, other) = (B256::repeat_byte(1), B256::repeat_byte(2));
    let element = U256::from(7);

    let insert = ISet::insertCall { set, element };
    assert!(
        invoke(&mut journal, &insert, CallScheme::StaticCall)
            .output
            .is_empty()
    );
    assert!(call::<bool>(&mut journal, &insert));

    // The pending insert is visible in its own set only.
    assert!(contains(&mut journal, set, 7));
    assert!(!contains(&mut journal, other, 7));
    assert_eq!(length(&mut journal, set), U256::from(1));
    assert_eq!(length(&mut journal, other), U256::ZERO);
    assert_eq!(elements(&mut journal, set), [7]);
    assert!(elements(&mut journal, other).is_empty());
}

#[test]
fn pending_additions_cancel_and_re_add_at_new_positions() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let set = B256::repeat_byte(1);

    assert!(stage(&mut journal, set, 7, true));
    assert!(!stage(&mut journal, set, 7, true)); // Already pending.
    assert!(stage(&mut journal, set, 8, true));
    assert!(stage(&mut journal, set, 7, false)); // Cancelled.
    assert!(!stage(&mut journal, set, 7, false)); // Absent: no-op.
    assert!(!contains(&mut journal, set, 7));
    assert_eq!(length(&mut journal, set), U256::from(1));
    assert!(stage(&mut journal, set, 7, true)); // Re-added at a new position.

    // Length and traversal skip the cancelled position.
    assert_eq!(length(&mut journal, set), U256::from(2));
    assert_eq!(elements(&mut journal, set), [8, 7]);

    let id = SetKeyCodec::derive_key(set.into());
    let delta = with_internals(&mut journal, |internals| SetAdapter::delta(internals, OWNER, id)).unwrap();
    assert_eq!(
        delta.added.slots(),
        &[None, Some(U256::from(8)), Some(U256::from(7))]
    );
    assert!(delta.removed.is_empty());
}

#[test]
fn committed_elements_are_staged_for_removal_and_restored_by_insert() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let set = B256::repeat_byte(1);
    commit(&mut journal, set, &[9, 10, 11]);
    assert_eq!(elements(&mut journal, set), [9, 10, 11]);

    assert!(!stage(&mut journal, set, 9, true)); // Already committed.
    assert!(stage(&mut journal, set, 9, false));
    assert!(!stage(&mut journal, set, 9, false)); // Already pending removal.
    assert!(stage(&mut journal, set, 11, false));
    assert!(!contains(&mut journal, set, 9));
    assert_eq!(length(&mut journal, set), U256::from(1));
    assert_eq!(elements(&mut journal, set), [10]);

    // Pending additions follow the committed list.
    assert!(stage(&mut journal, set, 12, true));
    assert_eq!(elements(&mut journal, set), [10, 12]);

    // Re-inserting only cancels the pending removal.
    assert!(stage(&mut journal, set, 9, true));
    assert!(contains(&mut journal, set, 9));
    assert_eq!(length(&mut journal, set), U256::from(3));
    assert_eq!(elements(&mut journal, set), [9, 10, 12]);

    let id = SetKeyCodec::derive_key(set.into());
    let delta = with_internals(&mut journal, |internals| SetAdapter::delta(internals, OWNER, id)).unwrap();
    assert_eq!(delta.added.slots(), &[Some(U256::from(12))]);
    assert_eq!(delta.removed.iter().copied().collect::<Vec<_>>(), [U256::from(11)]);
}

#[test]
fn element_zero_is_a_regular_member() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let set = B256::repeat_byte(1);
    commit(&mut journal, set, &[5, 0, 6]);

    // 5's successor is 0, stored under flag 1; absent elements still read as absent.
    assert!(contains(&mut journal, set, 5));
    assert!(contains(&mut journal, set, 0));
    assert!(!contains(&mut journal, set, 4));
    assert_eq!(elements(&mut journal, set), [5, 0, 6]);

    // 0 can be removed and added like any other element.
    assert!(stage(&mut journal, set, 0, false));
    assert_eq!(elements(&mut journal, set), [5, 6]);
    assert!(stage(&mut journal, set, 0, true));
    assert_eq!(elements(&mut journal, set), [5, 0, 6]);
}

#[test]
fn pending_logs_keep_one_element_per_word() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let set = B256::repeat_byte(1);
    commit(&mut journal, set, &[30, 10, 20]);
    for (element, insert) in [(7, true), (8, true), (7, false), (20, false), (10, false)] {
        assert!(stage(&mut journal, set, element, insert));
    }

    // Each log's index 0 is its byte length; elements follow one per word.
    let id = SetKeyCodec::derive_key(set.into());
    let read = |journal: &mut Journal<EmptyDB>, property, index| {
        let key = SetKeyCodec::delta(property, id).key(index).unwrap();
        with_internals(journal, |internals| internals.read(OWNER, key).unwrap())
    };
    let added = (0..3).map(|i| read(&mut journal, SetProperty::DeltaAdded, i)).collect::<Vec<_>>();
    assert_eq!(added, [U256::from(64), U256::ZERO, U256::from(8)]); // 7 cancelled: zero word.
    let cancelled = (0..2).map(|i| read(&mut journal, SetProperty::DeltaCancelled, i)).collect::<Vec<_>>();
    assert_eq!(cancelled, [U256::from(32), U256::from(0b1)]); // Position 0 is cancelled.
    let removals = (0..3).map(|i| read(&mut journal, SetProperty::PendingRemovals, i)).collect::<Vec<_>>();
    assert_eq!(removals, [U256::from(64), U256::from(10), U256::from(20)]); // Ascending.
}
