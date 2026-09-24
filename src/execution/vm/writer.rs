use super::cache::VmCache;
use crate::committer::traits::TransitionWriter;
use crate::crdt::state::{Delta, Marker, Tracked, Value};
use crate::store::traits::StoreError;

pub struct VmCacheWriter<'a, K> {
    vm_cache: VmCache<'a, K>,
}

impl<'a, K> VmCacheWriter<'a, K> {
    pub fn new(vm_cache: VmCache<'a, K>) -> Self {
        Self { vm_cache }
    }

    pub fn into_inner(self) -> VmCache<'a, K> {
        self.vm_cache
    }
}

/// A write-only store implementation for the execution cache.
/// Useful for using the execution cache as the fallback store for another execution cache.
impl<'a, K> TransitionWriter<K, Value<'static>> for VmCacheWriter<'a, K>
where
    K: std::hash::Hash + Eq + Clone,
{
    fn flush(&mut self, updates: Vec<(K, Value<'static>)>) -> Result<(), StoreError> {
        for (key, value) in updates {
            if matches!(value, Value::Marker(Marker::Deleted)) {
                self.vm_cache.get_or_populate_tracked(&key).delete()?;
                continue;
            }
            if matches!(
                value,
                Value::Marker(Marker::None | Marker::Missing | Marker::Stripped)
            ) {
                return Err(StoreError::ValueCannotBeStripped);
            }

            if self.vm_cache.get_or_populate_tracked(&key).is_deleted() {
                return Err(StoreError::ValueCannotBeRecreated);
            }

            // Update
            if self.vm_cache.exists(&key) {
                let delta = value.delta();
                if !matches!(delta, Delta::None) {
                    let tracked = self.vm_cache.get_or_populate_tracked(&key);
                    tracked.add_delta(delta)?;
                    continue;
                }
            }
            self.vm_cache
                .cache
                .insert(key, Tracked::new_owned_existing(value, self.vm_cache.id));
        }
        Ok(())
    }
}
