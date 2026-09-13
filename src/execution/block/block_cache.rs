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

impl<'a, K> FallbackStore<'a, K, Value<'a>> for BlockCache<'a, K>
where
    K: std::hash::Hash + Eq,
{
    fn contains_key(&self, key: &K) -> bool {
        self.cache.contains_key(key) || self.fallback.map_or(false, |f| f.contains_key(key))
    }

    fn get(&self, key: &K) -> Option<&Value<'a>> {
        self.cache
            .get(key)
            .or_else(|| self.fallback.and_then(|f| f.get(key)))
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
