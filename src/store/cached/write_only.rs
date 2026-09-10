use super::cached::CachedStore;
use crate::store::store::{Error, WriteOnlyStore};

impl<'a, K, V> WriteOnlyStore<'a, K, V> for CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash,
{
    fn stage(&mut self, updates: Vec<(K, V)>) -> Result<(), Error> {
        Ok(())
    }
    fn commit_batch(&mut self, updates: Vec<(K, V)>) {
        for (key, value) in updates {
            if self.cache.contains_key(&key) {
                continue;
            }
            self.cache.insert(key, value);
        }
    }
}
