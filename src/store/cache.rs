// use crate::crdt::state::{Delta, Tracked, Value};
use crate::store::traits::FallbackStore;

pub struct CachedStore<'a, K, V> {
    pub(super) cache: quick_cache::unsync::Cache<K, V>,
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

    fn get(&self, key: &K) -> Option<&V> {
        match self.cache.peek(key) {
            Some(value) => Some(value),
            None => self
                .fallback
                .as_ref()
                .and_then(|fallback| (**fallback).get_untracked(key)),
        }
    }
}

impl<'a, K, V> FallbackStore<'a, K, V> for CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
    V: Send + Sync,
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

    fn get_untracked(&self, key: &K) -> Option<&V> {
        self.get(key)
    }
}
