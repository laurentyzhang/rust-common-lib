use super::cache::VmStateCache;
use crate::committer::traits::TransitionWriter;
use crate::execution::cache::Entry;
use crate::store::traits::StoreError;

pub struct VmStateCacheWriter<'a, K, T = ()> {
    vm_cache: VmStateCache<'a, K, T>,
}

impl<'a, K, T> VmStateCacheWriter<'a, K, T> {
    pub fn new(vm_cache: VmStateCache<'a, K, T>) -> Self {
        Self { vm_cache }
    }

    pub fn into_inner(self) -> VmStateCache<'a, K, T> {
        self.vm_cache
    }
}

/// A write-only store implementation for the execution cache.
/// Useful for using the execution cache as the fallback store for another execution cache.
impl<'a, K, T: Clone> TransitionWriter<K, Entry<'static, T>> for VmStateCacheWriter<'a, K, T>
where
    K: std::hash::Hash + Eq + Clone,
{
    /// Applies resolved updates to the VM cache while preserving access/write tracking.
    fn flush(&mut self, updates: Vec<(K, Entry<'static, T>)>) -> Result<(), StoreError> {
        for (key, value) in updates {
            // `None` is a deletion. Loading the tracked entry first preserves whether
            // the key originally existed, which is needed by conflict detection.
            if value.is_missing() {
                self.vm_cache.try_get_or_populate_tracked(&key)?.remove()?;
                continue;
            }

            // A value deleted from this cache cannot be recreated in the same lifecycle.
            if self
                .vm_cache
                .try_get_or_populate_tracked(&key)?
                .is_deleted()
            {
                return Err(StoreError::ValueCannotBeRecreated);
            }

            // The key already exists locally or in the fallback store.
            if self.vm_cache.exists(&key) {
                // Numeric and other CRDT updates may carry an operation such as
                // `Add` or `Sub`. Merge that operation with the existing value.
                if let Some(delta) = value.delta() {
                    let tracked = self.vm_cache.try_get_or_populate_tracked(&key)?;
                    tracked.add_delta(delta)?;
                    continue;
                }

                // A value without a delta is an initialization value.
                self.vm_cache
                    .try_get_or_populate_tracked(&key)?
                    .replace(value)?;
                continue;
            }

            // The key is missing, so create it and record a tracked write.
            self.vm_cache
                .try_get_or_populate_tracked(&key)?
                .set(value)?;
        }
        Ok(())
    }
}
