use super::cached::CachedStore;
use crate::store::store::ReadOnlyStore;

impl<'a, K, V> ReadOnlyStore<'a, K, V> for CachedStore<'a, K, V>
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
