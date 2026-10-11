use super::cache::VmStateCache;
use crate::committer::traits::TransitionWriter;
use crate::crdt::state::{CrdtDelta, value::CrdtValue};
use crate::store::traits::StoreError;

pub struct VmStateCacheWriter<'a, K> {
    vm_cache: VmStateCache<'a, K>,
}

impl<'a, K> VmStateCacheWriter<'a, K> {
    pub fn new(vm_cache: VmStateCache<'a, K>) -> Self {
        Self { vm_cache }
    }

    pub fn into_inner(self) -> VmStateCache<'a, K> {
        self.vm_cache
    }
}

/// A write-only store implementation for the execution cache.
/// Useful for using the execution cache as the fallback store for another execution cache.
impl<'a, K> TransitionWriter<K, CrdtValue<'static>> for VmStateCacheWriter<'a, K>
where
    K: std::hash::Hash + Eq + Clone,
{
    /// Applies resolved updates to the VM cache while preserving access/write tracking.
    fn flush(&mut self, updates: Vec<(K, CrdtValue<'static>)>) -> Result<(), StoreError> {
        for (key, value) in updates {
            // `None` is a deletion. Loading the tracked entry first preserves whether
            // the key originally existed, which is needed by conflict detection.
            if matches!(value, CrdtValue::None) {
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
                let delta = value.delta();

                // Numeric and other CRDT updates may carry an operation such as
                // `Add` or `Sub`. Merge that operation with the existing value.
                if !matches!(delta, CrdtDelta::None) {
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
