use crate::committer::traits::TransitionWriter;
use crate::execution::{BlockCache, BlockCacheWriter};

use crate::crdt::{
    state::{Marker, Numeric, Value},
    uint64::U64,
};
use crate::store::traits::FallbackStore;
use crate::store::{CachedStoreWriter, cache::CachedStore};

fn write_cached<'a, K, V>(
    store: CachedStore<'a, K, V>,
    updates: Vec<(K, V)>,
) -> CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash,
{
    let mut writer = CachedStoreWriter::new(store);
    writer.flush(updates).unwrap();
    writer.into_inner()
}

fn write_block<'a, K>(
    cache: BlockCache<'a, K>,
    updates: Vec<(K, Value<'static>)>,
) -> BlockCache<'a, K>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
{
    let mut writer = BlockCacheWriter::new(cache);
    writer.flush(updates).unwrap();
    writer.into_inner()
}

fn numeric_u64(number: u64) -> Value<'static> {
    Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64 {
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
    assert!(cache.contains_key(&7));
    assert!((cache.get(&7)) == Some(&original));

    cache = write_block(cache, vec![(7, replacement.clone())]);
    assert!(cache.contains_key(&7));
    assert!((cache.get(&7)) == Some(&replacement));
    assert!((fallback.get(&7)) == Some(&original));

    cache = write_block(cache, vec![(7, Value::Marker(Marker::Deleted))]);
    assert!(!cache.contains_key(&7));
    assert!(cache.get(&7).is_none());
    assert!((fallback.get(&7)) == Some(&original));

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

    assert!(cache.contains_key(&1));
    assert!(cache.contains_key(&2));
    assert!((cache.get(&1)) == Some(&value));
    assert!((cache.get(&2)) == Some(&numeric_u64(3)));
    assert!((cache.get(&3)) == None);
}

#[test]
fn deletion_cannot_be_followed_by_recreation_in_the_same_batch() {
    let cache = BlockCache::new();
    let mut writer = BlockCacheWriter::new(cache);

    assert_eq!(
        writer.flush(vec![
            (7, Value::Marker(Marker::Deleted)),
            (7, numeric_u64(42)),
        ]),
        Err(crate::store::StoreError::ValueCannotBeRecreated)
    );
}
