use super::*;
use crate::store::cache::{ByWeight, PersistentStore};
use crate::store::traits::Committer;

#[test]
fn mdbx_state_serves_the_cache_with_sets_expanded() {
    use crate::execution::set::{KeyCodec, Property};
    use alloy_evm::revm::state::{Account, EvmStorageSlot};

    let directory = tempfile::tempdir().unwrap();
    let mdbx = LocalMdbxStore::open(directory.path()).unwrap();
    let owner = Address::repeat_byte(0x11);
    let set = [1u8; 6];
    let code = Bytecode::new_raw(vec![0x60, 0x00].into());
    let mut account = Account::from(AccountInfo {
        balance: U256::from(5),
        nonce: 2,
        code_hash: code.hash_slow(),
        account_id: None,
        code: Some(code.clone()),
    });
    // The store holds SET compact, as the list 10 → 20.
    let slot = |value: u64| EvmStorageSlot::new(U256::from(value), Default::default());
    account.storage.insert(KeyCodec::length(set), slot(2));
    account.storage.insert(KeyCodec::head(set), slot(10));
    account.storage.insert(KeyCodec::tail(set), slot(20));
    account.storage.insert(
        KeyCodec::element(Property::Node, set, U256::from(10)),
        slot(20),
    );
    let state = EvmState::from_iter([(owner, account)]);
    mdbx.write(&encode(&state).collect::<Vec<_>>()).unwrap();

    let cache = PersistentStore::<8>::new(1 << 20, ByWeight, Default::default(), Some(&mdbx));
    let info = cache.basic_ref(owner).unwrap().unwrap();
    assert_eq!(
        (info.balance, info.nonce, info.code_hash),
        (U256::from(5), 2, code.hash_slow())
    );
    assert_eq!(
        cache
            .code_by_hash_ref(code.hash_slow())
            .unwrap()
            .original_bytes(),
        code.original_bytes()
    );
    // The cache reads SET expanded.
    assert_eq!(
        cache.storage_ref(owner, KeyCodec::indexed(set)).unwrap(),
        U256::from(1)
    );
    let position = KeyCodec::element(Property::Position, set, U256::from(20));
    assert_eq!(cache.storage_ref(owner, position).unwrap(), U256::from(2));
}

#[test]
fn mdbx_state_replaces_and_deletes_slots() {
    use alloy_evm::revm::state::{Account, EvmStorageSlot};

    let directory = tempfile::tempdir().unwrap();
    let mdbx = LocalMdbxStore::open(directory.path()).unwrap();
    let owner = Address::repeat_byte(0x22);
    let mut committer = LocalMdbxCommitter::new(LocalMdbxWriter {
        mdbx: &mdbx,
        cache: PersistentStore::new(1 << 20, ByWeight, Default::default(), Some(&mdbx)),
    });
    let mut batch = 0;
    let mut write = |key: u64, value: u64| {
        let mut account = Account::from(AccountInfo::default());
        account.storage.insert(
            U256::from(key),
            EvmStorageSlot::new(U256::from(value), Default::default()),
        );
        batch += 1;
        committer.buffer(batch, EvmState::from_iter([(owner, account)]));
        committer.stage(vec![batch]).unwrap();
        committer.flush().unwrap();
    };

    write(1, 7);
    write(2, 8);
    write(1, 9);
    write(1, 0);
    assert_eq!(mdbx.storage_ref(owner, U256::from(1)).unwrap(), U256::ZERO);
    assert_eq!(
        mdbx.storage_ref(owner, U256::from(2)).unwrap(),
        U256::from(8)
    );
}

#[test]
fn mdbx_committer_compacts_execution_format_changes_across_batches() {
    use crate::common::codec::CodecError;
    use crate::execution::ApplyDelta;
    use crate::execution::evm::state::{StateDb, StateOverlay};
    use crate::execution::set::{Applier, Delta, KeyCodec, Property};
    use crate::execution::workload::ID;
    use alloy_evm::revm::state::{Account, EvmStorage, EvmStorageSlot};

    let directory = tempfile::tempdir().unwrap();
    let mdbx = LocalMdbxStore::open(directory.path()).unwrap();
    let owner = Address::repeat_byte(0x33);
    let set = [1u8; 6];
    let node = |element: u64| KeyCodec::element(Property::Node, set, U256::from(element));

    // MDBX starts with SET as the list 10 → 20.
    let mut account = Account::from(AccountInfo {
        balance: U256::from(1),
        ..Default::default()
    });
    let slot = |value: u64| EvmStorageSlot::new(U256::from(value), Default::default());
    account.storage.insert(KeyCodec::length(set), slot(2));
    account.storage.insert(KeyCodec::head(set), slot(10));
    account.storage.insert(KeyCodec::tail(set), slot(20));
    account.storage.insert(node(10), slot(20));
    let state = EvmState::from_iter([(owner, account)]);
    mdbx.write(&encode(&state).collect::<Vec<_>>()).unwrap();

    // The cache in front of MDBX is written only with accepted batches, after MDBX.
    let mut committer = LocalMdbxCommitter::new(LocalMdbxWriter {
        mdbx: &mdbx,
        cache: PersistentStore::new(1 << 20, ByWeight, Default::default(), Some(&mdbx)),
    });
    let run = |cache: &PersistentStore<'_>, property: Property, element: u64| {
        let mut block_db = StateDb::with_fallback(
            StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas),
            Some(cache),
        );
        let mut slots = EvmStorage::default();
        KeyCodec::delta(property, set)
            .encode(&vec![U256::from(element)], |key, word| {
                slots.insert(
                    key,
                    EvmStorageSlot::new_changed(U256::ZERO, word, Default::default()),
                );
                Ok::<_, CodecError>(())
            })
            .unwrap();
        let mut delta = Delta::default();
        Applier::merge_delta_from(&mut delta, ID::default(), &slots).unwrap();
        Applier::apply(&mut block_db, owner, delta).unwrap();
        block_db.into_inner()
    };
    let cached_length = |committer: &LocalMdbxCommitter<'_>| {
        committer
            .writer()
            .cache
            .storage_ref(owner, KeyCodec::length(set))
            .unwrap()
    };

    // Batch 1 removes 20 and is accepted; batch 2 adds 30 and is rejected: MDBX holds the list 10.
    let first = run(&committer.writer().cache, Property::PendingRemovals, 20);
    committer.buffer(1, first);
    let rejected = run(&committer.writer().cache, Property::DeltaAdded, 30);
    committer.buffer(2, rejected);
    assert_eq!(cached_length(&committer), U256::from(2));
    committer.stage(vec![1]).unwrap();
    committer.flush().unwrap();
    assert_eq!(
        mdbx.storage_ref(owner, KeyCodec::head(set)).unwrap(),
        U256::from(10)
    );
    assert_eq!(
        mdbx.storage_ref(owner, KeyCodec::tail(set)).unwrap(),
        U256::from(10)
    );
    assert_eq!(mdbx.storage_ref(owner, node(10)).unwrap(), U256::ZERO);
    assert_eq!(
        mdbx.storage_ref(owner, KeyCodec::indexed(set)).unwrap(),
        U256::ZERO
    );
    assert_eq!(
        mdbx.storage_ref(owner, KeyCodec::length(set)).unwrap(),
        U256::from(1)
    );
    assert_eq!(cached_length(&committer), U256::from(1));

    // Batch 3 adds 20 back: MDBX holds 10 → 20 again.
    let third = run(&committer.writer().cache, Property::DeltaAdded, 20);
    committer.buffer(3, third);
    committer.stage(vec![3]).unwrap();
    committer.flush().unwrap();
    assert_eq!(mdbx.storage_ref(owner, node(10)).unwrap(), U256::from(20));
    assert_eq!(
        mdbx.storage_ref(owner, KeyCodec::tail(set)).unwrap(),
        U256::from(20)
    );
    assert_eq!(
        mdbx.storage_ref(owner, KeyCodec::length(set)).unwrap(),
        U256::from(2)
    );
    assert_eq!(cached_length(&committer), U256::from(2));
}
