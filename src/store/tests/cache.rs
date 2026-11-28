use std::collections::HashMap;

use super::*;
use crate::common::codec::CodecError;
use crate::execution::ApplyDelta;
use crate::execution::evm::state::StateOverlay;
use crate::execution::set::{Applier, Delta, KeyCodec, Property};
use crate::execution::workload::ID;
use alloy_evm::revm::state::{Account, AccountInfo, EvmStorage, EvmStorageSlot};

struct MapStore(HashMap<u64, u64>);

impl PhysicalStore<u64, u64> for MapStore {
    fn get(&self, key: &u64) -> Result<Option<u64>, StoreError> {
        Ok(self.0.get(key).copied())
    }
}

#[test]
fn consuming_and_rebuilding_cached_store_preserves_cache_and_fallback() {
    let mut fallback = MapStore(HashMap::from([(1, 10)]));
    let store = CachedStore::<_, _>::new(4, UnitWeighter, Default::default(), Some(&fallback));

    assert_eq!(store.get_with(&1, |value| *value), Ok(Some(10)));
    let cache = store.into_cache();

    fallback.0.insert(2, 20);
    let store = CachedStore::from_cache(cache, Some(&fallback));

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

/// Runs a batch over `cache` that adds or removes `element` in SET, then compacts it and writes the
/// changes and the list into the cache, as the MDBX committer does; returns what goes to the store.
fn batch(cache: &mut PersistentStore<'_, 8>, property: Property, element: u64) -> EvmState {
    let mut block_db = StateDb::with_fallback(
        StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas),
        Some(&*cache),
    );
    let mut slots = EvmStorage::default();
    KeyCodec::delta(property, SET)
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
    Applier::apply(&mut block_db, OWNER, delta).unwrap();

    let merged = block_db.into_inner();
    let mut persisted = merged.clone();
    let slots = std::mem::take(&mut persisted.get_mut(&OWNER).unwrap().storage);
    let mut db = StateDb::with_fallback(
        StateOverlay::new(merged, StateOverlay::copy_balance_nonce),
        Some(&*cache),
    );
    persisted.get_mut(&OWNER).unwrap().storage =
        set::Converter::compact(&mut db, OWNER, &slots).unwrap();
    let merged = db.into_inner();
    cache.write(entries(merged)).unwrap();
    cache.write(entries(persisted.clone())).unwrap();
    persisted
}

/// Reads `key` of OWNER from a persisted state.
fn persisted_slot(state: &EvmState, key: U256) -> Option<U256> {
    state[&OWNER]
        .storage
        .get(&key)
        .map(|slot| slot.present_value)
}

#[test]
fn the_cache_keeps_the_execution_format_and_the_store_gets_the_list() {
    let store = store();
    let mut cache = PersistentStore::<8>::new(1 << 20, ByWeight, Default::default(), Some(&store));
    let node = |element: u64| KeyCodec::element(Property::Node, SET, U256::from(element));

    // Batch 1 removes 20: the store gets the list 10, and the cache stays converted.
    let first = batch(&mut cache, Property::PendingRemovals, 20);
    assert_eq!(
        persisted_slot(&first, KeyCodec::head(SET)),
        Some(U256::from(10))
    );
    assert_eq!(
        persisted_slot(&first, KeyCodec::tail(SET)),
        Some(U256::from(10))
    );
    assert_eq!(persisted_slot(&first, node(10)), Some(U256::ZERO));
    assert_eq!(persisted_slot(&first, KeyCodec::indexed(SET)), None);
    assert_eq!(
        cache.storage_ref(OWNER, KeyCodec::indexed(SET)).unwrap(),
        U256::from(1)
    );

    // Batch 2 adds 20 back: 10 → 20 must be written again, as the store holds 10 → 0.
    let second = batch(&mut cache, Property::DeltaAdded, 20);
    assert_eq!(persisted_slot(&second, node(10)), Some(U256::from(20)));
    assert_eq!(
        persisted_slot(&second, KeyCodec::tail(SET)),
        Some(U256::from(20))
    );
    assert_eq!(
        persisted_slot(&second, KeyCodec::length(SET)),
        Some(U256::from(2))
    );
}

#[test]
fn a_write_skips_sets_the_cache_does_not_hold() {
    let store = store();
    let mut cache = PersistentStore::<1>::new(1 << 20, ByWeight, Default::default(), Some(&store));
    let cached = |cache: &PersistentStore<'_, 1>, key: &CacheKey| {
        cache.shards[0].read().unwrap().contains_key(key)
    };
    let length = KeyCodec::length(SET);

    // A Set not cached is skipped, as only part of it is written; plain entries are kept.
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
    assert!(cached(&cache, &CacheKey::Slot(OWNER, U256::from(100))));

    // Its next read expands the whole Set from the store.
    assert_eq!(
        cache.storage_ref(OWNER, KeyCodec::indexed(SET)).unwrap(),
        U256::from(1)
    );
    assert_eq!(cache.storage_ref(OWNER, length).unwrap(), U256::from(2));
}
