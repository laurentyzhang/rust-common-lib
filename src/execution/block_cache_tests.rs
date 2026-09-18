use super::BlockCache;
use crate::crdt::{
    state::{Numeric, Value},
    uint64::U64,
};
use crate::store::cached::CachedStore;
use crate::store::traits::{FallbackStore, WriteOnlyStore};

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
    fallback.commit(vec![(7, original.clone())]);

    let mut cache = BlockCache::new_with_fallback(Some(&fallback));
    assert!(cache.contains_key(&7));
    assert!((cache.get(&7)) == Some(&original));

    cache.stage(vec![(7, replacement.clone())]).unwrap();
    assert!(cache.contains_key(&7));
    assert!((cache.get(&7)) == Some(&replacement));
    assert!((fallback.get(&7)) == Some(&original));

    cache.stage(vec![(7, Value::None)]).unwrap();
    assert!(!cache.contains_key(&7));
    assert!(cache.get(&7).is_none());
    assert!((fallback.get(&7)) == Some(&original));

    cache.stage(vec![(7, replacement.clone())]).unwrap();
    assert!(cache.contains_key(&7));
    assert!((cache.get(&7)) == Some(&replacement));
}

#[test]
fn staged_updates_are_available_to_the_cache_even_without_fallback() {
    let mut cache = BlockCache::new();
    let value = numeric_u64(99);

    cache
        .stage(vec![(1, value.clone()), (2, numeric_u64(3))])
        .unwrap();

    assert!(cache.contains_key(&1));
    assert!(cache.contains_key(&2));
    assert!((cache.get(&1)) == Some(&value));
    assert!((cache.get(&2)) == Some(&numeric_u64(3)));
    assert!((cache.get(&3)) == None);
}
