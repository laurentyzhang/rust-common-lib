// use crate::crdt::state::{Delta, Error};
use crate::crdt::state::Value;
use crate::store::execution::cache::ExecutionCache;
use crate::store::store::{Error, WriteOnlyStore};

impl<'a, K> WriteOnlyStore<'a, K, Value> for ExecutionCache<'a, K>
where
    K: std::hash::Hash + Eq,
{
    fn stage(&mut self, _updates: Vec<(K, Value)>) -> Result<(), Error> {
        Ok(())
    }

    /// Apply local deltas for the supplied keys; incoming values are unused.
    fn commit_batch(&mut self, updates: Vec<(K, Value)>) {
        for (key, _) in updates {
            if let Some(tracked) = self.cache.get_mut(&key) {
                tracked.apply_delta();
            }
        }
    }
}
