use crate::store::traits::StoreError;

/// Applies committed updates to the entries of a `CachedStore`.
/// It wraps the bare cache (see `CachedStore::into_cache`), so it never holds the fallback borrow.
pub struct CachedStoreWriter<K, V> {
    cache: quick_cache::sync::Cache<K, V>,
}

impl<K, V> CachedStoreWriter<K, V> {
    pub fn new(cache: quick_cache::sync::Cache<K, V>) -> Self {
        Self { cache }
    }

    pub fn into_inner(self) -> quick_cache::sync::Cache<K, V> {
        self.cache
    }
}

impl<K, V> CachedStoreWriter<K, V>
where
    K: Eq + std::hash::Hash + Clone,
    V: Clone,
{
    pub fn flush(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError> {
        for (key, value) in updates {
            self.cache.insert(key, value);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests/writer.rs"]
mod tests;
