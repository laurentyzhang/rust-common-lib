use crate::store::traits::{FallbackStore, StoreError};

pub struct CachedStore<'a, K, V> {
    pub(super) cache: quick_cache::sync::Cache<K, V>,
    fallback: Option<&'a dyn FallbackStore<K, V>>,
}

impl<'a, K, V> CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash,
    V: Clone,
{
    pub fn new(capacity: usize, fallback: Option<&'a dyn FallbackStore<K, V>>) -> Self {
        Self {
            cache: quick_cache::sync::Cache::new(capacity),
            fallback,
        }
    }
}

impl<'a, K, V> CachedStore<'a, K, V> {
    pub fn into_cache(self) -> quick_cache::sync::Cache<K, V> {
        self.cache
    }

    pub fn from_cache(
        cache: quick_cache::sync::Cache<K, V>,
        fallback: Option<&'a dyn FallbackStore<K, V>>,
    ) -> Self {
        Self { cache, fallback }
    }
}

impl<K, V> FallbackStore<K, V> for CachedStore<'_, K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    fn contains_key(&self, key: &K) -> Result<bool, StoreError> {
        if self.cache.contains_key(key) {
            return Ok(true);
        }
        match self.fallback {
            Some(fallback) => fallback.contains_key(key),
            None => Ok(false),
        }
    }

    fn get(&self, key: &K) -> Result<Option<V>, StoreError> {
        if let Some(value) = self.cache.get(key) {
            return Ok(Some(value));
        }

        let Some(fallback) = self.fallback else {
            return Ok(None);
        };
        let Some(value) = fallback.get(key)? else {
            return Ok(None);
        };
        self.cache.insert(key.clone(), value.clone());
        Ok(Some(value))
    }

    fn get_untracked(&self, key: &K) -> Result<Option<V>, StoreError> {
        self.get(key)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    struct MapStore(HashMap<u64, u64>);

    impl FallbackStore<u64, u64> for MapStore {
        fn get(&self, key: &u64) -> Result<Option<u64>, StoreError> {
            Ok(self.0.get(key).copied())
        }
    }

    #[test]
    fn consuming_and_rebuilding_cached_store_preserves_cache_and_fallback() {
        let mut fallback = MapStore(HashMap::from([(1, 10)]));
        let store = CachedStore::new(4, Some(&fallback));

        assert_eq!(store.get(&1), Ok(Some(10)));
        let cache = store.into_cache();

        fallback.0.insert(2, 20);
        let store = CachedStore::from_cache(cache, Some(&fallback));

        assert_eq!(store.get(&1), Ok(Some(10)));
        assert_eq!(store.get(&2), Ok(Some(20)));
    }
}
