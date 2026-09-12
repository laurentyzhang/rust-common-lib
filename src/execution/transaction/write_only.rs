// use crate::crdt::state::{Delta, Error};
use crate::crdt::state::Value;
use crate::execution::transaction::cache::ExecutionCache;
use crate::store::traits::{Error, WriteOnlyStore};

/// A write-only store implementation for the execution cache.
/// Useful for using the execution cache as the fallback store for another execution cache.
impl<'a, K> WriteOnlyStore<K, Value<'a>> for ExecutionCache<'a, K>
where
    K: std::hash::Hash + Eq,
{
    fn stage(&mut self, updates: Vec<(K, Value<'a>)>) -> Result<(), Error> {
        // updates.iter_mut().for_each(|(key, value)| {
        // match value {

        // }

        // self.get(&key)

        // self.cache.insert(&key), value.clone());
        // });
        Ok(())
    }

    /// Apply local deltas for the supplied keys; incoming values are unused.
    fn commit_batch(&mut self, updates: Vec<(K, Value<'a>)>) {}
}
