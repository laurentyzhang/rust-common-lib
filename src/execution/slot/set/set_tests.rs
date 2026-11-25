use super::{Applier, Delta, Indexer, KeyCodec, Property, Converter};
use std::collections::BTreeMap;
use crate::execution::evm::storage::ApiStore;
use crate::execution::{
    ApiAddress, ApplyDelta, CallInput, CallOutput, CallRouter, CallScheme, FormatConverter, EvmStateCommitter, ISet,
    Recorder, SetHandler, StateDb,
};
use crate::planner::scheduler::ID;
use crate::store::traits::Committer;
use alloy_evm::{
    EvmInternals,
    revm::{
        Journal,
        context::{BlockEnv, CfgEnv, JournalTr, TxEnv},
        database::EmptyDB,
        state::{AccountStatus, EvmState},
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

/// Seeds `set` with committed `elements` as a linked list: Length, Head, Tail and Node links.
fn commit(journal: &mut Journal<EmptyDB>, set: B256, elements: &[u64]) {
    let id = KeyCodec::derive_key(set.into());
    with_internals(journal, |internals| {
        let write = |internals: &mut EvmInternals<'_>, key, value| internals.write(OWNER, key, value).unwrap();
        write(internals, KeyCodec::length(id), U256::from(elements.len()));
        write(internals, KeyCodec::head(id), U256::from(elements[0]));
        write(internals, KeyCodec::tail(id), U256::from(*elements.last().unwrap()));
        for pair in elements.windows(2) {
            // A Node of 0 stays empty.
            write(internals, KeyCodec::element(Property::Node, id, U256::from(pair[0])), U256::from(pair[1]));
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
    let set = KeyCodec::derive_key(U256::from(1));
    let element = U256::from(7);
    let key = KeyCodec::element(Property::Node, set, element).to_be_bytes::<32>();

    // [prefix:7][set ID:6][property:1][element hash:16][reserved:2].
    assert_eq!(&key[..4], b"ARCO");
    assert_eq!(&key[7..13], &set);
    assert_eq!(key[13], Property::Node as u8);
    assert_eq!(&key[14..30], &keccak256(element.to_be_bytes::<32>())[..16]);
    assert_eq!((key[30], key[31]), (0, 0));
    assert_eq!(
        KeyCodec::decode(U256::from_be_bytes(key)),
        Some((set, Property::Node, key[14..30].try_into().unwrap()))
    );

    // Log keys carry the index in the 16-byte field.
    let log = KeyCodec::delta(Property::DeltaAdded, set).key(3).unwrap().to_be_bytes::<32>();
    assert_eq!(&log[7..13], &set);
    assert_eq!(log[13], Property::DeltaAdded as u8);
    assert_eq!(&log[14..30], &3u128.to_be_bytes());
    assert_eq!((log[30], log[31]), (0, 0));

    // Undefined properties and non-zero reserved bytes are rejected.
    let mut bad = key;
    bad[13] = 13;
    assert_eq!(KeyCodec::decode(U256::from_be_bytes(bad)), None);
    for reserved in [30, 31] {
        let mut bad = key;
        bad[reserved] = 1;
        assert_eq!(KeyCodec::decode(U256::from_be_bytes(bad)), None);
    }
    assert_ne!(KeyCodec::length(set), KeyCodec::length(KeyCodec::derive_key(U256::from(2))));
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

    let id = KeyCodec::derive_key(set.into());
    let delta = with_internals(&mut journal, |internals| Indexer::delta(internals, OWNER, id)).unwrap();
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

    // Pending additions follow the indexed elements.
    assert!(stage(&mut journal, set, 12, true));
    assert_eq!(elements(&mut journal, set), [10, 12]);

    // Re-inserting only cancels the pending removal.
    assert!(stage(&mut journal, set, 9, true));
    assert!(contains(&mut journal, set, 9));
    assert_eq!(length(&mut journal, set), U256::from(3));
    assert_eq!(elements(&mut journal, set), [9, 10, 12]);

    let id = KeyCodec::derive_key(set.into());
    let delta = with_internals(&mut journal, |internals| Indexer::delta(internals, OWNER, id)).unwrap();
    assert_eq!(delta.added.slots(), &[Some(U256::from(12))]);
    assert_eq!(delta.removed.iter().copied().collect::<Vec<_>>(), [U256::from(11)]);
}

#[test]
fn element_zero_is_a_regular_member() {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    let set = B256::repeat_byte(1);
    commit(&mut journal, set, &[5, 0, 6]);

    // 5's Node is empty, read as 0 before Tail; absent elements still read as absent.
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
    let id = KeyCodec::derive_key(set.into());
    let read = |journal: &mut Journal<EmptyDB>, property, index| {
        let key = KeyCodec::delta(property, id).key(index).unwrap();
        with_internals(journal, |internals| internals.read(OWNER, key).unwrap())
    };
    let added = (0..3).map(|i| read(&mut journal, Property::DeltaAdded, i)).collect::<Vec<_>>();
    assert_eq!(added, [U256::from(64), U256::ZERO, U256::from(8)]); // 7 cancelled: zero word.
    let cancelled = (0..2).map(|i| read(&mut journal, Property::DeltaCancelled, i)).collect::<Vec<_>>();
    assert_eq!(cancelled, [U256::from(32), U256::from(0b1)]); // Position 0 is cancelled.
    let removals = (0..3).map(|i| read(&mut journal, Property::PendingRemovals, i)).collect::<Vec<_>>();
    assert_eq!(removals, [U256::from(64), U256::from(10), U256::from(20)]); // Ascending.
}

/// Returns a journal seeded with `set` committed as `elements`, standing in for one sequence.
fn sequence(set: B256, elements: &[u64]) -> Journal<EmptyDB> {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    commit(&mut journal, set, elements);
    exist(&mut journal);
    journal
}

/// Marks OWNER as an existing account, as a Set owner is a contract.
fn exist(journal: &mut Journal<EmptyDB>) {
    let account = journal.inner.state.get_mut(&OWNER).unwrap();
    account.status.remove(AccountStatus::LoadedAsNotExisting);
}

/// Copies OWNER's slots from `state` into a fresh journal, standing in for the next generation.
fn reload(state: &EvmState) -> Journal<EmptyDB> {
    let mut journal: Journal<EmptyDB> = Journal::new(EmptyDB::default());
    with_internals(&mut journal, |internals| {
        for (&key, slot) in &state[&OWNER].storage {
            internals.write(OWNER, key, slot.present_value).unwrap();
        }
    });
    exist(&mut journal);
    journal
}

/// Reads one of OWNER's slots from `db`.
fn slot(db: &mut StateDb, key: U256) -> U256 {
    db.read(OWNER, key).unwrap()
}

#[test]
fn sequences_merge_into_the_block_index_and_persist_as_the_committed_list() {
    let set = B256::repeat_byte(1);
    let id = KeyCodec::derive_key(set.into());
    let base = sequence(set, &[1, 2, 3]).inner.state;

    // Each sequence converts the same list and stages its own changes.
    let mut a = sequence(set, &[1, 2, 3]);
    assert!(stage(&mut a, set, 2, false));
    assert!(stage(&mut a, set, 4, true));
    let mut b = sequence(set, &[1, 2, 3]);
    assert!(stage(&mut b, set, 5, true));
    assert!(stage(&mut b, set, 6, true));
    assert!(stage(&mut b, set, 6, false));
    assert!(stage(&mut b, set, 1, false));

    let mut committer = EvmStateCommitter::with_target(base, EmptyDB::default());
    committer.buffer(ID { sequence: 1, ..ID::new(0, 1) }, a.inner.state);
    committer.buffer(ID { sequence: 2, ..ID::new(0, 2) }, b.inner.state);
    committer.flush().unwrap();
    let merged = committer.into_inner();

    // Pending slots are dropped; the index holds additions in sequence order, removals as holes.
    let pending = [Property::Added, Property::Removed, Property::DeltaAdded, Property::PendingRemovals];
    assert!(merged[&OWNER].storage.keys().all(|&key| {
        KeyCodec::decode(key).is_none_or(|(_, property, ..)| !pending.contains(&property))
    }));
    let mut next = reload(&merged);
    assert_eq!(elements(&mut next, set), vec![3, 4, 5]);
    assert_eq!(length(&mut next, set), U256::from(3));
    assert!(!contains(&mut next, set, 1) && !contains(&mut next, set, 6));

    // A later generation re-adds a removed element at the end of the index.
    assert!(stage(&mut next, set, 2, true));
    let mut committer = EvmStateCommitter::with_target(merged, EmptyDB::default());
    committer.buffer(ID::new(0, 3), next.inner.state);
    committer.flush().unwrap();

    // Block end relinks the live elements in index order and clears the index.
    let mut db = StateDb::new(committer.into_inner());
    let keys: Vec<_> = db.inner()[&OWNER].storage.keys().copied().collect();
    Converter::to_storage_format(&mut db, OWNER, &keys).unwrap();
    let node = |element: u64| KeyCodec::element(Property::Node, id, U256::from(element));
    assert_eq!(slot(&mut db, KeyCodec::head(id)), U256::from(3));
    assert_eq!(slot(&mut db, KeyCodec::tail(id)), U256::from(2));
    assert_eq!(slot(&mut db, KeyCodec::length(id)), U256::from(4));
    for (element, successor) in [(3, 4), (4, 5), (5, 2), (2, 0), (1, 0)] {
        assert_eq!(slot(&mut db, node(element)), U256::from(successor));
    }
    assert_eq!(slot(&mut db, KeyCodec::indexed(id)), U256::ZERO);
    assert!(db.inner()[&OWNER].storage.iter().all(|(&key, value)| {
        value.present_value.is_zero()
            || KeyCodec::decode(key).is_some_and(|(_, property, ..)| {
                matches!(property, Property::Length | Property::Head | Property::Tail | Property::Node)
            })
    }));
    assert_eq!(elements(&mut reload(db.inner()), set), vec![3, 4, 5, 2]);
}

#[test]
fn persist_links_element_zero_as_an_empty_node() {
    let set = B256::repeat_byte(1);
    let id = KeyCodec::derive_key(set.into());
    let mut journal = sequence(set, &[7]);
    assert!(stage(&mut journal, set, 0, true));
    assert!(stage(&mut journal, set, 8, true));
    let mut committer = EvmStateCommitter::with_target(sequence(set, &[7]).inner.state, EmptyDB::default());
    committer.buffer(ID::new(0, 1), journal.inner.state);
    committer.flush().unwrap();

    let mut db = StateDb::new(committer.into_inner());
    let keys: Vec<_> = db.inner()[&OWNER].storage.keys().copied().collect();
    Converter::to_storage_format(&mut db, OWNER, &keys).unwrap();
    // 7 -> 0 leaves Node[7] empty; 0 -> 8 is a plain link.
    let node = |element: u64| KeyCodec::element(Property::Node, id, U256::from(element));
    assert_eq!(slot(&mut db, node(7)), U256::ZERO);
    assert_eq!(slot(&mut db, node(0)), U256::from(8));
    assert_eq!(slot(&mut db, KeyCodec::head(id)), U256::from(7));
    assert_eq!(slot(&mut db, KeyCodec::tail(id)), U256::from(8));
    assert_eq!(elements(&mut reload(db.inner()), set), vec![7, 0, 8]);
}

#[test]
fn merged_deltas_apply_like_the_sequences_in_turn() {
    let set = B256::repeat_byte(1);
    let base = sequence(set, &[1, 2, 3]).inner.state;
    let mut a = sequence(set, &[1, 2, 3]);
    assert!(stage(&mut a, set, 2, false));
    assert!(stage(&mut a, set, 4, true));
    let mut b = sequence(set, &[1, 2, 3]);
    assert!(stage(&mut b, set, 5, true));
    assert!(stage(&mut b, set, 6, true));
    assert!(stage(&mut b, set, 6, false));
    assert!(stage(&mut b, set, 1, false));
    let states = [(ID { sequence: 1, ..ID::new(0, 1) }, &a.inner.state), (ID { sequence: 2, ..ID::new(0, 2) }, &b.inner.state)];

    // Applied one sequence at a time.
    let mut in_turn = StateDb::new(base.clone());
    for (id, state) in states {
        let mut delta = Delta::default();
        Applier::merge_delta(&mut delta, id, &state[&OWNER].storage).unwrap();
        Applier::apply(&mut in_turn, OWNER, delta).unwrap();
    }

    // Merged without a store, then applied once.
    let mut delta = Delta::default();
    for (id, state) in states {
        Applier::merge_delta(&mut delta, id, &state[&OWNER].storage).unwrap();
    }
    let mut once = StateDb::new(base);
    Applier::apply(&mut once, OWNER, delta).unwrap();

    // An absent slot reads as zero, so only non-zero values are compared.
    let values = |state: &EvmState| {
        state[&OWNER]
            .storage
            .iter()
            .filter(|(_, slot)| !slot.present_value.is_zero())
            .map(|(&key, slot)| (key, slot.present_value))
            .collect::<BTreeMap<_, _>>()
    };
    assert_eq!(values(once.inner()), values(in_turn.inner()));
    assert_eq!(elements(&mut reload(once.inner()), set), vec![3, 4, 5]);
}

#[test]
fn the_same_addition_from_two_states_is_indexed_once() {
    let set = B256::repeat_byte(1);
    let base = sequence(set, &[1, 2, 3]).inner.state;
    let mut a = sequence(set, &[1, 2, 3]);
    assert!(stage(&mut a, set, 4, true));
    let mut b = sequence(set, &[1, 2, 3]);
    assert!(stage(&mut b, set, 4, true));

    // Both states add 4; the lower ID's copy is kept and Length counts it once.
    let mut delta = Delta::default();
    Applier::merge_delta(&mut delta, ID { sequence: 2, ..ID::new(0, 2) }, &b.inner.state[&OWNER].storage).unwrap();
    Applier::merge_delta(&mut delta, ID { sequence: 1, ..ID::new(0, 1) }, &a.inner.state[&OWNER].storage).unwrap();
    let mut db = StateDb::new(base);
    Applier::apply(&mut db, OWNER, delta).unwrap();
    let mut next = reload(db.inner());
    assert_eq!(elements(&mut next, set), vec![1, 2, 3, 4]);
    assert_eq!(length(&mut next, set), U256::from(4));
}
