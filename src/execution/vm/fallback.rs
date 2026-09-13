// use crate::crdt::state::{Delta, Error};
use crate::crdt::state::Value;
use crate::execution::vm::vm_cache;
use crate::store::traits::FallbackStore;

/// VmCache as the fallback store for another VmCache,
/// allowing for a layered caching mechanism.
impl<'a, K> FallbackStore<'a, K, Value<'a>> for vm_cache::VmCache<'a, K>
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
            }

            None => self
                .fallback
                .as_ref()
                .and_then(|fallback| fallback.get(key)),
        }
    }
}
