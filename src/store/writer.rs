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
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::store::{FallbackStore, cache::CachedStore};

    struct MapStore(HashMap<u64, u64>);

    impl FallbackStore<u64, u64> for MapStore {
        fn get(&self, key: &u64) -> Result<Option<u64>, StoreError> {
            Ok(self.0.get(key).copied())
        }
    }

    impl MapStore {
        fn flush(&mut self, updates: Vec<(u64, u64)>) -> Result<(), StoreError> {
            self.0.extend(updates);
            Ok(())
        }
    }

    #[test]
    fn consume_commit_to_backend_and_cache_then_rebuild() {
        let mut backend = MapStore(HashMap::from([(1, 10)]));

        let store = CachedStore::new(8, Some(&backend));
        assert_eq!(store.get(&1), Ok(Some(10)));
        let mut cache_writer = CachedStoreWriter::new(store.into_cache());

        let updates = vec![(1, 11), (2, 20)];
        backend.flush(updates.clone()).unwrap();
        cache_writer.flush(updates).unwrap();

        let store = CachedStore::from_cache(cache_writer.into_inner(), Some(&backend));
        assert_eq!(store.get(&1), Ok(Some(11)));
        assert_eq!(store.get(&2), Ok(Some(20)));
        assert_eq!(backend.0.get(&1), Some(&11));
        assert_eq!(backend.0.get(&2), Some(&20));
    }
}
