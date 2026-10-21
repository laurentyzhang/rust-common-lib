use super::cache::BlockCache;
use crate::execution::cache::Entry;
use crate::store::traits::{FallbackStore, StoreError};

pub struct BlockCacheWriter<'a, K, T = ()> {
    block_cache: BlockCache<'a, K, T>,
}

impl<'a, K, T> BlockCacheWriter<'a, K, T> {
    pub fn new(cache: BlockCache<'a, K, T>) -> Self {
        Self { block_cache: cache }
    }

    pub fn into_inner(self) -> BlockCache<'a, K, T> {
        self.block_cache
    }
}

/// A write-only store implementation for the execution cache.
/// Useful for using the execution cache as the fallback store for another execution cache.
impl<'a, K, T> BlockCacheWriter<'a, K, T>
where
    K: std::hash::Hash + Eq + Clone + Send + Sync,
    T: Clone + Send + Sync,
{
    pub fn flush(&mut self, updates: Vec<(K, Entry<'static, T>)>) -> Result<(), StoreError> {
        let mut deleted = std::collections::HashSet::new();
        for (key, value) in &updates {
            if value.is_missing() {
                deleted.insert(key.clone());
            } else if deleted.contains(key)
                || self
                    .block_cache
                    .get_untracked(key)?
                    .is_some_and(|value| value.is_missing())
            {
                return Err(StoreError::ValueCannotBeRecreated);
            }
        }
        self.block_cache.cache.apply_batch(updates);
        Ok(())
    }
}
