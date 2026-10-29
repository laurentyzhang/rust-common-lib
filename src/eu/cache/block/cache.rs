use crate::collections;
use crate::execution::cache::Entry;
use crate::store::traits::FallbackStore;

/// A cache for storing all state transitions within a block.
pub struct BlockCache<'a, K, T = ()> {
    pub(in crate::execution) cache: collections::shard_map::ShardMap<K, Entry<'a, T>>,
    pub(super) fallback: Option<&'a dyn FallbackStore<K, Entry<'a, T>>>,
}

impl<'a, K, T> BlockCache<'a, K, T> {
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
    pub fn new_with_fallback(fallback: Option<&'a dyn FallbackStore<K, Entry<'a, T>>>) -> Self
    where
        K: std::hash::Hash + Eq,
    {
        Self {
            cache: collections::shard_map::ShardMap::new(16),
            fallback,
        }
    }
}

impl<'cache, 'value, K, T> FallbackStore<K, Entry<'value, T>> for BlockCache<'cache, K, T>
where
    K: std::hash::Hash + Eq + Send + Sync,
    T: Clone + Send + Sync,
    'cache: 'value,
{
    fn contains_key(&self, key: &K) -> Result<bool, crate::store::StoreError> {
        match self.cache.get(key) {
            Some(_) => Ok(true),
            None => match self.fallback {
                Some(fallback) => fallback.contains_key(key),
                None => Ok(false),
            },
        }
    }

    fn get(&self, key: &K) -> Result<Option<Entry<'value, T>>, crate::store::StoreError> {
        match self.cache.get(key) {
            Some(value) => Ok(Some(value.clone())),
            None => match self.fallback {
                Some(fallback) => fallback.get(key),
                None => Ok(None),
            },
        }
    }

    fn get_untracked(&self, key: &K) -> Result<Option<Entry<'value, T>>, crate::store::StoreError> {
        match self.cache.get(key) {
            Some(value) => Ok(Some(value.clone())),
            None => match self.fallback {
                Some(fallback) => fallback.get_untracked(key),
                None => Ok(None),
            },
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod block_cache_tests;
