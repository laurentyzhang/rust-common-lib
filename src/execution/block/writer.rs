use crate::committer::traits::TransitionWriter;
use crate::crdt::state::{Status, Value};
use crate::execution::block::cache::BlockCache;
use crate::store::traits::{FallbackStore, StoreError};

pub struct BlockCacheWriter<'a, K> {
    block_cache: BlockCache<'a, K>,
}

impl<'a, K> BlockCacheWriter<'a, K> {
    pub fn new(cache: BlockCache<'a, K>) -> Self {
        Self { block_cache: cache }
    }

    pub fn into_inner(self) -> BlockCache<'a, K> {
        self.block_cache
    }
}

/// A write-only store implementation for the execution cache.
/// Useful for using the execution cache as the fallback store for another execution cache.
impl<'a, K> TransitionWriter<K, Value<'static>> for BlockCacheWriter<'a, K>
where
    K: std::hash::Hash + Eq + Clone + Send + Sync,
{
    fn flush(&mut self, updates: Vec<(K, Value<'static>)>) -> Result<(), StoreError> {
        let mut deleted = std::collections::HashSet::new();
        for (key, value) in &updates {
            if matches!(value, Value::None) {
                deleted.insert(key.clone());
            } else if deleted.contains(key)
                || matches!(self.block_cache.get_untracked(key), Some(Value::None))
            {
                return Err(StoreError::ValueCannotBeRecreated);
            }
        }
        self.block_cache.cache.apply_batch(updates);
        Ok(())
    }
}
