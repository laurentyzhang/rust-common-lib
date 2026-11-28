use std::collections::HashMap;

use super::*;
use crate::execution::set::{Indexer, KeyCodec, Property};
use alloy_evm::revm::state::{Account, AccountInfo, EvmStorageSlot};

struct MapStore(HashMap<u64, u64>);

impl PhysicalStore<u64, u64> for MapStore {
    fn get(&self, key: &u64) -> Result<Option<u64>, StoreError> {
        Ok(self.0.get(key).copied())
    }
}

#[test]
fn consuming_and_rebuilding_cached_store_preserves_cache_and_fallback() {
    let mut fallback = MapStore(HashMap::from([(1, 10)]));
    let store =
        PhysicalCachedStore::<_, _>::new(4, UnitWeighter, Default::default(), Some(&fallback));

    assert_eq!(store.get_with(&1, |value| *value), Ok(Some(10)));
    let cache = store.into_cache();

    fallback.0.insert(2, 20);
    let store = PhysicalCachedStore::from_cache(cache, Some(&fallback));

    assert_eq!(store.get_with(&1, |value| *value), Ok(Some(10)));
    assert_eq!(store.get_with(&2, |value| *value), Ok(Some(20)));
}

const OWNER: Address = Address::repeat_byte(0x11);
const SET: [u8; 6] = [1; 6];

/// The store: OWNER exists and holds SET as the list 10 → 20.
fn store() -> StateDb {
    let mut account = Account::from(AccountInfo {
        balance: U256::from(1),
        ..Default::default()
    });
    let slot = |value: u64| EvmStorageSlot::new(U256::from(value), Default::default());
    account.storage.insert(KeyCodec::length(SET), slot(2));
    account.storage.insert(KeyCodec::head(SET), slot(10));
    account.storage.insert(KeyCodec::tail(SET), slot(20));
    account.storage.insert(
        KeyCodec::element(Property::Node, SET, U256::from(10)),
        slot(20),
    );
    StateDb::new(EvmState::from_iter([(OWNER, account)]))
}

/// Runs a batch over the cache that inserts or removes `element` in SET; its changes are pending
/// deltas over the committed Set.
fn batch(cache: &PersistentStore<'_>, insert: bool, element: u64) -> EvmState {
    let mut block_db = StateDb::with_fallback(
        StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas),
        Some(cache),
    );
    let element = U256::from(element);
    match insert {
        true => Indexer::insert(&mut block_db, OWNER, SET, element),
        false => Indexer::remove(&mut block_db, OWNER, SET, element),
    }
    .unwrap();
    block_db.into_inner()
}

/// Returns the keys of SET's cache entry by property, and the value of each.
fn cached_set(cache: &PersistentStore<'_>) -> Vec<(Property, U256)> {
    cache
        .get_with(&CacheKey::Set(OWNER, SET), |value| match value {
            CacheVal::Set(slots) => slots
                .iter()
                .filter_map(|(&key, slot)| KeyCodec::decode(key).map(|(_, property, _)| (property, slot.present_value)))
                .collect(),
            _ => Vec::new(),
        })
        .unwrap()
        .unwrap()
}

#[test]
fn the_cache_commits_deltas_into_its_index_without_holes() {
    let store = store();
    let mut committer = PhysicalStoreCacheCommitter::new(PersistentStore::new(
        1 << 20,
        ByWeight,
        Default::default(),
        Some(&store),
    ));
    let read = |committer: &PhysicalStoreCacheCommitter<'_>, key: U256| {
        committer.cache().storage_ref(OWNER, key).unwrap()
    };
    let index = KeyCodec::delta(Property::Index, SET);
    let position = |element: u64| KeyCodec::element(Property::Position, SET, U256::from(element));

    // Batch 1 removes 10; batch 2, rejected, adds 30.
    let first = batch(committer.cache(), false, 10);
    let rejected = batch(committer.cache(), true, 30);
    committer.buffer(1, first);
    committer.buffer(2, rejected);
    committer.stage(vec![1]).unwrap();
    committer.flush().unwrap();
    // The index is rebuilt without the hole, and the deltas are gone.
    assert_eq!(read(&committer, KeyCodec::length(SET)), U256::from(1));
    assert_eq!(read(&committer, index.key(0).unwrap()), U256::from(32));
    assert_eq!(read(&committer, index.key(1).unwrap()), U256::from(20));
    assert_eq!(read(&committer, position(20)), U256::from(1));
    assert_eq!(read(&committer, position(10)), U256::ZERO);
    assert!(cached_set(committer.cache()).iter().all(|(property, _)| {
        matches!(
            property,
            Property::Length | Property::Index | Property::Position | Property::Indexed
        )
    }));

    // Batches 3 and 4 run against the same base and both add: each element lands once, in batch
    // order.
    let third = batch(committer.cache(), true, 30);
    let fourth = batch(committer.cache(), true, 40);
    committer.buffer(3, third);
    committer.buffer(4, fourth);
    committer.stage(vec![3, 4]).unwrap();
    committer.flush().unwrap();
    assert_eq!(read(&committer, KeyCodec::length(SET)), U256::from(3));
    for (at, element) in [(1, 20), (2, 30), (3, 40)] {
        assert_eq!(read(&committer, index.key(at).unwrap()), U256::from(element));
        assert_eq!(read(&committer, position(element)), U256::from(at));
    }
}

#[test]
fn a_write_skips_sets_the_cache_does_not_hold() {
    let store = store();
    let mut cache = PersistentStore::<1>::new(1 << 20, ByWeight, Default::default(), Some(&store));
    let cached = |cache: &PersistentStore<'_, 1>, key: &CacheKey| {
        cache.shards[0].read().unwrap().contains_key(key)
    };
    let length = KeyCodec::length(SET);

    // An entry not cached is skipped, a Set or a plain slot alike: it loads from the store.
    let mut account = Account::from(AccountInfo::default());
    account.storage.insert(
        length,
        EvmStorageSlot::new(U256::from(2), Default::default()),
    );
    account.storage.insert(
        U256::from(100),
        EvmStorageSlot::new(U256::from(7), Default::default()),
    );
    cache
        .write(entries(EvmState::from_iter([(OWNER, account)])))
        .unwrap();
    assert!(!cached(&cache, &CacheKey::Set(OWNER, SET)));
    assert!(!cached(&cache, &CacheKey::Slot(OWNER, U256::from(100))));

    // Its next read expands the whole Set from the store.
    assert_eq!(
        cache.storage_ref(OWNER, KeyCodec::indexed(SET)).unwrap(),
        U256::from(1)
    );
    assert_eq!(cache.storage_ref(OWNER, length).unwrap(), U256::from(2));
}
