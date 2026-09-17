// use crate::crdt::state::{Delta, Tracked, Value};
use crate::store::traits::{FallbackStore, StoreError, WriteOnlyStore};

pub struct CachedStore<'a, K, V> {
    cache: quick_cache::unsync::Cache<K, V>,
    fallback: Option<&'a mut dyn FallbackStore<'a, K, V>>,
}

impl<'a, K, V> CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash,
{
    pub fn new(capacity: usize, fallback: Option<&'a mut dyn FallbackStore<'a, K, V>>) -> Self {
        Self {
            cache: quick_cache::unsync::Cache::new(capacity),
            fallback,
        }
    }
}

impl<'a, K, V> FallbackStore<'a, K, V> for CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash + Clone,
{
    fn contains_key(&self, key: &K) -> bool {
        self.cache.contains_key(key)
            || self
                .fallback
                .as_ref()
                .is_some_and(|fallback| (**fallback).contains_key(key))
    }

    fn get(&self, key: &K) -> Option<&V> {
        match self.cache.peek(key) {
            Some(value) => Some(value),
            None => self
                .fallback
                .as_ref()
                .and_then(|fallback| (**fallback).get(key)),
        }
    }
}

impl<'a, K, V> WriteOnlyStore<K, V> for CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash,
{
    fn stage(&mut self, _: Vec<(K, V)>) -> Result<(), StoreError> {
        Ok(())
    }

    fn commit(&mut self, updates: Vec<(K, V)>) {
        for (key, value) in updates {
            if self.cache.contains_key(&key) {
                continue;
            }
            self.cache.insert(key, value);
        }
    }
}
