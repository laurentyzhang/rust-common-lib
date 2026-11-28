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
    let mut committer = LocalMdbxCommitter::new(&mdbx, 1 << 20);
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
fn committers_apply_accepted_batches_to_mdbx_and_the_cache() {
    use crate::execution::evm::state::{StateDb, StateOverlay};
    use crate::execution::set::{Indexer, KeyCodec, Property};
    use crate::store::cache::PhysicalStoreCacheCommitter;
    use alloy_evm::revm::state::{Account, EvmStorageSlot};

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

    // MDBX's committer and the cache's get the same batches and flush in parallel.
    let mut physical = LocalMdbxCommitter::new(&mdbx, 1 << 20);
    let mut cache = PhysicalStoreCacheCommitter::new(PersistentStore::new(
        1 << 20,
        ByWeight,
        Default::default(),
        Some(&mdbx),
    ));
    let run = |cache: &PersistentStore<'_>, insert: bool, element: u64| {
        let mut block_db = StateDb::with_fallback(
            StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas),
            Some(cache),
        );
        let element = U256::from(element);
        match insert {
            true => Indexer::insert(&mut block_db, owner, set, element),
            false => Indexer::remove(&mut block_db, owner, set, element),
        }
        .unwrap();
        block_db.into_inner()
    };
    let commit = |physical: &mut LocalMdbxCommitter<'_>,
                  cache: &mut PhysicalStoreCacheCommitter<'_>,
                  batches: Vec<(u64, EvmState)>,
                  accepted: Vec<u64>| {
        for (batch, changes) in batches {
            physical.buffer(batch, changes.clone());
            cache.buffer(batch, changes);
        }
        physical.stage(accepted.clone()).unwrap();
        cache.stage(accepted).unwrap();
        let (stored, cached) = rayon::join(|| physical.flush(), || cache.flush());
        stored.unwrap();
        cached.unwrap();
    };
    let stored = |key: U256| mdbx.storage_ref(owner, key).unwrap();
    let cached = |cache: &PhysicalStoreCacheCommitter<'_>, key: U256| {
        cache.cache().storage_ref(owner, key).unwrap()
    };

    // Batch 1 removes 20 and is accepted; batch 2 adds 30 and is rejected: MDBX holds the list 10.
    let first = run(cache.cache(), false, 20);
    let rejected = run(cache.cache(), true, 30);
    commit(&mut physical, &mut cache, vec![(1, first), (2, rejected)], vec![1]);
    assert_eq!(stored(KeyCodec::head(set)), U256::from(10));
    assert_eq!(stored(KeyCodec::tail(set)), U256::from(10));
    assert_eq!(stored(node(10)), U256::ZERO);
    assert_eq!(stored(KeyCodec::indexed(set)), U256::ZERO);
    assert_eq!(stored(KeyCodec::length(set)), U256::from(1));
    assert_eq!(cached(&cache, KeyCodec::length(set)), U256::from(1));

    // Batch 3 adds 20 back: MDBX holds 10 → 20 again.
    let third = run(cache.cache(), true, 20);
    commit(&mut physical, &mut cache, vec![(3, third)], vec![3]);
    assert_eq!(stored(node(10)), U256::from(20));
    assert_eq!(stored(KeyCodec::tail(set)), U256::from(20));
    assert_eq!(stored(KeyCodec::length(set)), U256::from(2));
    assert_eq!(cached(&cache, KeyCodec::length(set)), U256::from(2));

    // Batches 4 and 5 run against the same base and both add; a cache that never read SET skips
    // it and loads it whole from MDBX afterwards.
    let mut cold = PhysicalStoreCacheCommitter::new(PersistentStore::new(
        1 << 20,
        ByWeight,
        Default::default(),
        Some(&mdbx),
    ));
    let fourth = run(cache.cache(), true, 40);
    let fifth = run(cache.cache(), true, 50);
    for (batch, changes) in [(4, &fourth), (5, &fifth)] {
        cold.buffer(batch, changes.clone());
    }
    commit(&mut physical, &mut cache, vec![(4, fourth), (5, fifth)], vec![4, 5]);
    cold.stage(vec![4, 5]).unwrap();
    cold.flush().unwrap();
    for (element, successor) in [(10, 20), (20, 40), (40, 50)] {
        assert_eq!(stored(node(element)), U256::from(successor));
    }
    assert_eq!(stored(KeyCodec::tail(set)), U256::from(50));
    assert_eq!(stored(KeyCodec::length(set)), U256::from(4));
    for committer in [&cache, &cold] {
        assert_eq!(cached(committer, KeyCodec::length(set)), U256::from(4));
        for (at, element) in [(1, 10), (2, 20), (3, 40), (4, 50)] {
            let position = KeyCodec::element(Property::Position, set, U256::from(element));
            assert_eq!(cached(committer, position), U256::from(at));
        }
    }
}

#[test]
fn an_account_created_over_a_funded_address_loads_its_slots_from_mdbx() {
    use crate::store::cache::PhysicalStoreCacheCommitter;
    use alloy_evm::revm::state::{Account, EvmStorageSlot};

    let directory = tempfile::tempdir().unwrap();
    let mdbx = LocalMdbxStore::open(directory.path()).unwrap();
    let owner = Address::repeat_byte(0x44);
    let key = U256::from(1);

    // The address holds a balance before any contract is deployed there, and the cache reads it.
    let funded = Account::from(AccountInfo {
        balance: U256::from(5),
        ..Default::default()
    });
    mdbx.write(&encode(&EvmState::from_iter([(owner, funded)])).collect::<Vec<_>>())
        .unwrap();
    let mut physical = LocalMdbxCommitter::new(&mdbx, 1 << 20);
    let mut cache = PhysicalStoreCacheCommitter::new(PersistentStore::new(
        1 << 20,
        ByWeight,
        Default::default(),
        Some(&mdbx),
    ));
    assert!(cache.cache().basic_ref(owner).unwrap().is_some());

    // A block creates a contract there whose constructor writes the slot.
    let mut created = Account::from(AccountInfo {
        balance: U256::from(5),
        ..Default::default()
    });
    created.mark_created();
    created.mark_touch();
    created.storage.insert(
        key,
        EvmStorageSlot::new_changed(U256::ZERO, U256::from(7), Default::default()),
    );
    let changes = EvmState::from_iter([(owner, created)]);
    physical.buffer(1, changes.clone());
    cache.buffer(1, changes);
    physical.stage(vec![1]).unwrap();
    cache.stage(vec![1]).unwrap();
    let (stored, cached) = rayon::join(|| physical.flush(), || cache.flush());
    stored.unwrap();
    cached.unwrap();

    // Later reads load the slot from MDBX instead of treating the account as created.
    assert_eq!(mdbx.storage_ref(owner, key).unwrap(), U256::from(7));
    assert_eq!(cache.cache().storage_ref(owner, key).unwrap(), U256::from(7));
}
