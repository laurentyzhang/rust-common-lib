// use crate::crdt::state::{Delta, Error};
use crate::crdt::state::Value;
use crate::execution::transaction::cache;
use crate::store::traits::FallbackStore;
// use std::collections::HashMap;

/// ExecutionCache as the fallback store for another ExecutionCache,
/// allowing for a layered caching mechanism.
impl<'a, K> FallbackStore<'a, K, Value<'a>> for cache::ExecutionCache<'a, K>
where
    K: std::hash::Hash + Eq,
{
    /// Check for a live value locally or in the fallback.
    fn contains_key(&self, key: &K) -> bool {
        self.exists(key)
    }

    /// Read without tracking; local tombstones hide fallback values.
    fn get(&self, key: &K) -> Option<&Value<'a>> {
        match self.cache.get(key) {
            Some(tracked) => {
                if tracked.is_live() {
                    Some(tracked.value())
                } else {
                    None
                }
                // Some(tracked.value())
            }

            None => self
                .fallback
                .as_ref()
                .and_then(|fallback| fallback.get(key)),
        }
    }
}
