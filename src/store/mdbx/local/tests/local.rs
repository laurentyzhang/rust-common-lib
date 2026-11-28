use super::*;
use crate::store::{IdentityCodec, cache::CachedStore};

#[test]
fn mdbx_writer_persists_updates_for_cached_store_reads() {
    let directory = tempfile::tempdir().unwrap();
    let mut mdbx = MdbxStore::open(directory.path(), IdentityCodec, IdentityCodec).unwrap();
    mdbx.flush(vec![(b"key".to_vec(), b"value".to_vec())])
        .unwrap();
    let store = CachedStore::new(4, Some(&mdbx));
    assert_eq!(
        store.get(&b"key".to_vec()).unwrap(),
        Some(b"value".to_vec())
    );

    drop(store);
    drop(mdbx);

    let reopened = MdbxStore::open(directory.path(), IdentityCodec, IdentityCodec).unwrap();
    assert_eq!(
        reopened.get(&b"key".to_vec()).unwrap(),
        Some(b"value".to_vec())
    );
}

#[test]
fn consume_commit_to_mdbx_and_cache_then_rebuild() {
    use crate::store::CachedStoreWriter;

    let directory = tempfile::tempdir().unwrap();
    let mut mdbx = MdbxStore::open(directory.path(), IdentityCodec, IdentityCodec).unwrap();
    let key = b"key".to_vec();

    let store = CachedStore::new(4, Some(&mdbx));
    assert_eq!(store.get(&key).unwrap(), None);
    let mut cache_writer = CachedStoreWriter::new(store.into_cache());

    let updates = vec![(key.clone(), b"value".to_vec())];
    mdbx.flush(updates.clone()).unwrap();
    cache_writer.flush(updates).unwrap();

    let store = CachedStore::from_cache(cache_writer.into_inner(), Some(&mdbx));
    assert_eq!(store.get(&key).unwrap(), Some(b"value".to_vec()));
    assert_eq!(mdbx.get(&key).unwrap(), Some(b"value".to_vec()));
}

#[test]
fn evm_key_and_raw_value_round_trip() {
    use super::super::key::{Key, StoreKeyCodec};

    let directory = tempfile::tempdir().unwrap();
    let mut mdbx: MdbxStore<Key, Vec<u8>, _, _> =
        MdbxStore::open(directory.path(), StoreKeyCodec, IdentityCodec).unwrap();
    let key = Key::Account(alloy_primitives::Address::repeat_byte(0x11));
    let value = vec![42];
    mdbx.flush(vec![(key.clone(), value.clone())]).unwrap();
    assert_eq!(mdbx.get(&key).unwrap(), Some(value));
}

#[test]
fn mdbx_state_serves_the_cache_with_sets_expanded() {
    use crate::execution::evm::state::sharded::Sharded;
    use crate::execution::set::{KeyCodec, Property};
    use crate::store::cache::CacheDb;
    use alloy_evm::revm::state::{Account, EvmStorageSlot};

    let directory = tempfile::tempdir().unwrap();
    let mdbx = MdbxState::open(directory.path()).unwrap();
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
    account
        .storage
        .insert(KeyCodec::element(Property::Node, set, U256::from(10)), slot(20));
    let mut committer = MdbxCommitter::new(&mdbx);
    committer.buffer(1, EvmState::from_iter([(owner, account)]));
    committer.stage(vec![1]).unwrap();
    committer.flush().unwrap();

    let shards = Sharded::<8>::new();
    let cache = CacheDb::new(&shards, &mdbx);
    let info = cache.basic_ref(owner).unwrap().unwrap();
    assert_eq!((info.balance, info.nonce, info.code_hash), (U256::from(5), 2, code.hash_slow()));
    assert_eq!(
        cache.code_by_hash_ref(code.hash_slow()).unwrap().original_bytes(),
        code.original_bytes()
    );
    // The cache reads SET expanded.
    assert_eq!(cache.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::from(1));
    let position = KeyCodec::element(Property::Position, set, U256::from(20));
    assert_eq!(cache.storage_ref(owner, position).unwrap(), U256::from(2));
}

#[test]
fn mdbx_state_replaces_and_deletes_slots() {
    use alloy_evm::revm::state::{Account, EvmStorageSlot};

    let directory = tempfile::tempdir().unwrap();
    let mdbx = MdbxState::open(directory.path()).unwrap();
    let owner = Address::repeat_byte(0x22);
    let mut committer = MdbxCommitter::new(&mdbx);
    let mut block = 0;
    let mut write = |key: u64, value: u64| {
        let mut account = Account::from(AccountInfo::default());
        account.storage.insert(
            U256::from(key),
            EvmStorageSlot::new(U256::from(value), Default::default()),
        );
        block += 1;
        committer.buffer(block, EvmState::from_iter([(owner, account)]));
        committer.stage(vec![block]).unwrap();
        committer.flush().unwrap();
    };

    write(1, 7);
    write(2, 8);
    write(1, 9);
    write(1, 0);
    assert_eq!(mdbx.storage_ref(owner, U256::from(1)).unwrap(), U256::ZERO);
    assert_eq!(mdbx.storage_ref(owner, U256::from(2)).unwrap(), U256::from(8));
}

#[test]
fn mdbx_committer_compacts_execution_format_changes_across_blocks() {
    use crate::common::codec::CodecError;
    use crate::execution::ApplyDelta;
    use crate::execution::evm::state::store::OverlayWrite;
    use crate::execution::evm::state::{StateDb, StateOverlay, sharded::Sharded};
    use crate::execution::set::{Applier, Delta, KeyCodec, Property};
    use crate::execution::workload::ID;
    use crate::store::cache::CacheDb;
    use alloy_evm::revm::state::{Account, EvmStorage, EvmStorageSlot};

    let directory = tempfile::tempdir().unwrap();
    let mdbx = MdbxState::open(directory.path()).unwrap();
    let mut committer = MdbxCommitter::new(&mdbx);
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
    committer.buffer(0, EvmState::from_iter([(owner, account)]));
    committer.stage(vec![0]).unwrap();
    committer.flush().unwrap();

    // The execution side: its cache keeps only the execution format of each block's changes.
    let mut cache = Sharded::<8>::new();
    let mut run = |property: Property, element: u64| -> EvmState {
        let cache_db = CacheDb::new(&cache, &mdbx);
        cache_db.basic_ref(owner).unwrap();
        let mut block_db = StateDb::with_fallback(
            StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas),
            Some(&cache_db),
        );
        let mut slots = EvmStorage::default();
        KeyCodec::delta(property, set)
            .encode(&vec![U256::from(element)], |key, word| {
                slots.insert(key, EvmStorageSlot::new_changed(U256::ZERO, word, Default::default()));
                Ok::<_, CodecError>(())
            })
            .unwrap();
        let mut delta = Delta::default();
        Applier::merge_delta_from(&mut delta, ID::default(), &slots).unwrap();
        Applier::apply(&mut block_db, owner, delta).unwrap();
        let changes = block_db.into_inner();
        drop(cache_db);
        cache.write_accounts(changes.clone());
        changes
    };

    // Block 1 removes 20: MDBX holds the list 10.
    committer.buffer(1, run(Property::PendingRemovals, 20));
    committer.stage(vec![1]).unwrap();
    committer.flush().unwrap();
    assert_eq!(mdbx.storage_ref(owner, KeyCodec::head(set)).unwrap(), U256::from(10));
    assert_eq!(mdbx.storage_ref(owner, KeyCodec::tail(set)).unwrap(), U256::from(10));
    assert_eq!(mdbx.storage_ref(owner, node(10)).unwrap(), U256::ZERO);
    assert_eq!(mdbx.storage_ref(owner, KeyCodec::indexed(set)).unwrap(), U256::ZERO);

    // Block 2 adds 20 back: MDBX holds 10 → 20 again.
    committer.buffer(2, run(Property::DeltaAdded, 20));
    committer.stage(vec![2]).unwrap();
    committer.flush().unwrap();
    assert_eq!(mdbx.storage_ref(owner, node(10)).unwrap(), U256::from(20));
    assert_eq!(mdbx.storage_ref(owner, KeyCodec::tail(set)).unwrap(), U256::from(20));
    assert_eq!(mdbx.storage_ref(owner, KeyCodec::length(set)).unwrap(), U256::from(2));
}
