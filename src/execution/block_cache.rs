use crate::collections;
use crate::crdt::state::Value;
use crate::store::traits::{FallbackStore, WriteOnlyStore};

/// A cache for storing all state transitions within a block.
pub struct BlockCache<'a, K> {
    pub(super) cache: collections::shard_map::ShardMap<K, Value<'a>>,
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
    K: std::hash::Hash + Eq,
    'cache: 'value,
{
    fn contains_key(&self, key: &K) -> bool {
        match self.cache.get(key) {
            Some(Value::None) => false,
            Some(_) => true,
            None => self
                .fallback
                .is_some_and(|fallback| fallback.contains_key(key)),
        }
    }

    fn get(&self, key: &K) -> Option<&Value<'value>> {
        match self.cache.get(key) {
            Some(Value::None) => None,
            Some(value) => Some(value),
            None => self.fallback.and_then(|fallback| fallback.get(key)),
        }
    }
}

impl<'a, K> WriteOnlyStore<K, Value<'a>> for BlockCache<'a, K>
where
    K: std::hash::Hash + Eq + Send + Sync,
{
    fn stage(
        &mut self,
        updates: Vec<(K, Value<'a>)>,
    ) -> Result<(), crate::store::traits::StoreError> {
        self.cache.apply_batch(updates);
        Ok(())
    }

    fn commit(&mut self, _: Vec<(K, Value<'a>)>) {} // Place holder
}

#[cfg(test)]
#[path = "block_cache_tests.rs"]
mod block_cache_tests;
