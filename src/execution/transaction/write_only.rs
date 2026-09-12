// use crate::crdt::state::{Delta, Error};
use crate::crdt::state::Value;
use crate::execution::transaction::cache::ExecutionCache;
use crate::store::FallbackStore;
use crate::store::traits::{Error, WriteOnlyStore};

/// A write-only store implementation for the execution cache.
/// Useful for using the execution cache as the fallback store for another execution cache.
impl<'a, K> WriteOnlyStore<K, Value<'a>> for ExecutionCache<'a, K>
where
    K: std::hash::Hash + Eq,
{
    fn stage(&mut self, updates: Vec<(K, Value)>) -> Result<(), Error> {
        // updates.iter().for_each(|(key, value)| {
        //     if !self.contains_key(key) {
        //         self.create(key, value);
        //     }
        // });
        Ok(())
    }

    /// Apply local deltas for the supplied keys; incoming values are unused.
    fn commit_batch(&mut self, updates: Vec<(K, Value<'a>)>) {}
}
