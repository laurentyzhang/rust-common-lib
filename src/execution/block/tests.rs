use crate::committer::traits::TransitionWriter;
use crate::execution::{BlockCache, BlockCacheWriter};

use crate::crdt::{
    state::{Numeric, CrdtValue},
    uint64::U64,
};
use crate::store::traits::FallbackStore;
use crate::store::{CachedStoreWriter, cache::CachedStore};

fn write_cached<'a, K, V>(
    store: CachedStore<'a, K, V>,
    updates: Vec<(K, V)>,
) -> CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash + Clone,
    V: Clone,
{
    let mut writer = CachedStoreWriter::new(store.into_cache());
    writer.flush(updates).unwrap();
    CachedStore::from_cache(writer.into_inner(), None)
}

fn write_block<'a, K>(
    cache: BlockCache<'a, K>,
    updates: Vec<(K, CrdtValue<'static>)>,
) -> BlockCache<'a, K>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
{
    let mut writer = BlockCacheWriter::new(cache);
    writer.flush(updates).unwrap();
    writer.into_inner()
}

fn numeric_u64(number: u64) -> CrdtValue<'static> {
    CrdtValue::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64 {
        value: number,
        ..U64::default()
    })))
}

#[test]
fn fallback_values_are_visible_and_stage_updates_override_them() {
    let original = numeric_u64(17);
    let replacement = numeric_u64(42);
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(7, original.clone())]);

    let mut cache = BlockCache::new_with_fallback(Some(&fallback));
    assert!(cache.contains_key(&7).unwrap());
    assert!((cache.get(&7)) == Ok(Some(original.clone())));

    cache = write_block(cache, vec![(7, replacement.clone())]);
    assert!(cache.contains_key(&7).unwrap());
    assert!((cache.get(&7)) == Ok(Some(replacement.clone())));
    assert!((fallback.get(&7)) == Ok(Some(original.clone())));

    cache = write_block(cache, vec![(7, CrdtValue::None)]);
    assert!(cache.contains_key(&7).unwrap());
    assert!(matches!(cache.get(&7), Ok(Some(CrdtValue::None))));
    assert!((fallback.get(&7)) == Ok(Some(original.clone())));

    let mut writer = BlockCacheWriter::new(cache);
    assert_eq!(
        writer.flush(vec![(7, replacement)]),
        Err(crate::store::StoreError::ValueCannotBeRecreated)
    );
}

#[test]
fn staged_updates_are_available_to_the_cache_even_without_fallback() {
    let mut cache = BlockCache::new();
    let value = numeric_u64(99);

    cache = write_block(cache, vec![(1, value.clone()), (2, numeric_u64(3))]);

    assert!(cache.contains_key(&1).unwrap());
    assert!(cache.contains_key(&2).unwrap());
    assert!((cache.get(&1)) == Ok(Some(value)));
    assert!((cache.get(&2)) == Ok(Some(numeric_u64(3))));
    assert!((cache.get(&3)) == Ok(None));
}

#[test]
fn deletion_cannot_be_followed_by_recreation_in_the_same_batch() {
    let cache = BlockCache::new();
    let mut writer = BlockCacheWriter::new(cache);

    assert_eq!(
        writer.flush(vec![(7, CrdtValue::None), (7, numeric_u64(42)),]),
        Err(crate::store::StoreError::ValueCannotBeRecreated)
    );
}
