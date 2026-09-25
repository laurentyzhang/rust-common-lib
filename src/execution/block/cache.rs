use crate::collections;
use crate::crdt::state::{Marker, Value};
use crate::store::traits::FallbackStore;

/// A cache for storing all state transitions within a block.
pub struct BlockCache<'a, K> {
    pub(in crate::execution) cache: collections::shard_map::ShardMap<K, Value<'a>>,
    pub(super) fallback: Option<&'a dyn FallbackStore<'a, K, Value<'a>>>,
}

impl<'a, K> BlockCache<'a, K> {
    /// Creates a new `BlockCache` with an optional fallback store.
    pub fn new() -> Self
    where
        K: std::hash::Hash + Eq,
    {
        Self {
            cache: collections::shard_map::ShardMap::new(16),
            fallback: None,
        }
    }

    /// Creates a new `BlockCache` with a fallback store.
    pub fn new_with_fallback(fallback: Option<&'a dyn FallbackStore<'a, K, Value<'a>>>) -> Self
    where
        K: std::hash::Hash + Eq,
    {
        Self {
            cache: collections::shard_map::ShardMap::new(16),
            fallback,
        }
    }
}

impl<'store, 'cache, 'value, K> FallbackStore<'store, K, Value<'value>> for BlockCache<'cache, K>
where
    K: std::hash::Hash + Eq + Send + Sync,
    'cache: 'value,
{
    fn contains_key(&self, key: &K) -> bool {
        match self.cache.get(key) {
            Some(Value::Marker(Marker::Missing | Marker::Deleted | Marker::Stripped)) => false,
            Some(_) => true,
            None => self
                .fallback
                .is_some_and(|fallback| fallback.contains_key(key)),
        }
    }

    fn get(&self, key: &K) -> Option<&Value<'value>> {
        match self.cache.get(key) {
            Some(Value::Marker(Marker::Missing | Marker::Deleted | Marker::Stripped)) => None,
            Some(value) => Some(value),
            None => self.fallback.and_then(|fallback| fallback.get(key)),
        }
    }

    fn get_raw(&self, key: &K) -> Option<&Value<'value>> {
        match self.cache.get(key) {
            Some(value) => Some(value),
            None => self.fallback.and_then(|fallback| fallback.get_raw(key)),
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod block_cache_tests;
