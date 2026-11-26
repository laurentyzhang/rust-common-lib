use std::collections::HashMap;

use super::*;
use crate::common::codec::CodecError;
use crate::execution::evm::state::sharded::update;
use crate::execution::ApplyDelta;
use crate::execution::set::{Applier, Delta, KeyCodec, Property};
use crate::execution::workload::ID;
use alloy_evm::revm::state::{Account, AccountInfo, EvmStorage, EvmStorageSlot};

struct MapStore(HashMap<u64, u64>);

impl FallbackStore<u64, u64> for MapStore {
    fn get(&self, key: &u64) -> Result<Option<u64>, StoreError> {
        Ok(self.0.get(key).copied())
    }
}

#[test]
fn consuming_and_rebuilding_cached_store_preserves_cache_and_fallback() {
    let mut fallback = MapStore(HashMap::from([(1, 10)]));
    let store = CachedStore::new(4, Some(&fallback));

    assert_eq!(store.get(&1), Ok(Some(10)));
    let cache = store.into_cache();

    fallback.0.insert(2, 20);
    let store = CachedStore::from_cache(cache, Some(&fallback));

    assert_eq!(store.get(&1), Ok(Some(10)));
    assert_eq!(store.get(&2), Ok(Some(20)));
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
    account
        .storage
        .insert(KeyCodec::element(Property::Node, SET, U256::from(10)), slot(20));
    StateDb::new(EvmState::from_iter([(OWNER, account)]))
}

/// Runs one block over `cache` that adds or removes `element` in SET, then persists it as
/// `Block::run` does; returns what goes to the store.
fn block(cache: &mut Sharded<8>, store: &StateDb, property: Property, element: u64) -> EvmState {
    let cache_db = StateDb::with_fallback(
        StateOverlay::new(&*cache, StateOverlay::copy_balance_nonce),
        Some(store),
    );
    cache_db.basic_ref(OWNER).unwrap();
    let mut block_db = StateDb::with_fallback(
        StateOverlay::new(EvmState::default(), StateOverlay::add_balance_nonce_deltas),
        Some(&cache_db),
    );
    let mut slots = EvmStorage::default();
    KeyCodec::delta(property, SET)
        .encode(&vec![U256::from(element)], |key, word| {
            slots.insert(key, EvmStorageSlot::new_changed(U256::ZERO, word, Default::default()));
            Ok::<_, CodecError>(())
        })
        .unwrap();
    let mut delta = Delta::default();
    Applier::merge_delta(&mut delta, ID::default(), &slots).unwrap();
    Applier::apply(&mut block_db, OWNER, delta).unwrap();

    let merged = block_db.into_inner();
    let persisted = to_storage(merged.clone(), &cache_db).unwrap().into_inner();
    drop(cache_db);
    update(cache, merged, persisted.clone());
    persisted
}

/// Reads `key` of OWNER from a persisted state.
fn persisted_slot(state: &EvmState, key: U256) -> Option<U256> {
    state[&OWNER].storage.get(&key).map(|slot| slot.present_value)
}

#[test]
fn the_cache_keeps_the_execution_format_and_the_store_gets_the_list() {
    let store = store();
    let mut cache = Sharded::<8>::new_cache();
    let node = |element: u64| KeyCodec::element(Property::Node, SET, U256::from(element));

    // Block 1 removes 20: the store gets the list 10, and the cache stays converted.
    let first = block(&mut cache, &store, Property::PendingRemovals, 20);
    assert_eq!(persisted_slot(&first, KeyCodec::head(SET)), Some(U256::from(10)));
    assert_eq!(persisted_slot(&first, KeyCodec::tail(SET)), Some(U256::from(10)));
    assert_eq!(persisted_slot(&first, node(10)), Some(U256::ZERO));
    assert_eq!(persisted_slot(&first, KeyCodec::indexed(SET)), Some(U256::ZERO));
    assert_eq!(
        cache.read_slot(OWNER, KeyCodec::indexed(SET), |slot| slot.map(|slot| slot.present_value)),
        Some(U256::from(1))
    );

    // Block 2 adds 20 back: 10 → 20 must be written again, as the store holds 10 → 0.
    let second = block(&mut cache, &store, Property::DeltaAdded, 20);
    assert_eq!(persisted_slot(&second, node(10)), Some(U256::from(20)));
    assert_eq!(persisted_slot(&second, KeyCodec::tail(SET)), Some(U256::from(20)));
    assert_eq!(persisted_slot(&second, KeyCodec::length(SET)), Some(U256::from(2)));
}
