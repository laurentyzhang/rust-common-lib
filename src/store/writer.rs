use crate::committer::traits::TransitionWriter;
use crate::store::traits::StoreError;

pub struct CachedStoreWriter<'a, K, V> {
    store: crate::store::cache::CachedStore<'a, K, V>,
}

impl<'a, K, V> CachedStoreWriter<'a, K, V> {
    pub fn new(store: crate::store::cache::CachedStore<'a, K, V>) -> Self {
        Self { store }
    }

    pub fn into_inner(self) -> crate::store::cache::CachedStore<'a, K, V> {
        self.store
    }
}

impl<'a, K, V> TransitionWriter<K, V> for CachedStoreWriter<'a, K, V>
where
    K: Eq + std::hash::Hash,
{
    fn flush(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError> {
        for (key, value) in updates {
            if self.store.cache.contains_key(&key) {
                continue;
            }
            self.store.cache.insert(key, value);
        }
        Ok(())
    }
}
