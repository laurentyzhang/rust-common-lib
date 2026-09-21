use crate::committer::traits::TransitionWriter;
use crate::crdt::state::Value;
use crate::execution::block::cache::BlockCache;
use crate::store::traits::StoreError;

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
    K: std::hash::Hash + Eq + Clone + Send,
{
    fn flush(&mut self, updates: Vec<(K, Value<'static>)>) -> Result<(), StoreError> {
        self.block_cache.cache.apply_batch(updates);
        Ok(())
    }
}
