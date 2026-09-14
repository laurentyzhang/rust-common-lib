use crate::crdt::state::{Delta, StateError};
use crate::crdt::state::{Tracked, Value};
use crate::store::traits::FallbackStore;
use crate::store::traits::StoreError;
use crate::store::traits::WriteOnlyStore;
use std::collections::HashMap;

pub struct VmCache<'a, K> {
    pub(super) cache: HashMap<K, Tracked<'a>>,
    pub(super) fallback: Option<&'a dyn FallbackStore<'a, K, Value<'a>>>,
}

impl<'a, K: std::hash::Hash + Eq> VmCache<'a, K> {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            fallback: None,
        }
    }

    /// Create an empty cache backed by a read-only fallback.
    pub fn new_with_fallback(fallback: &'a dyn FallbackStore<'a, K, Value<'a>>) -> Self {
        Self {
            cache: HashMap::new(),
            fallback: Some(fallback),
        }
    }

    pub fn size(&self) -> u64 {
        self.cache.len() as u64
    }

    /// Populate the cache if needed and record a tracked read.
    ///
    /// Use `(&mut cache).get(key)` to select this method when `FallbackStore`
    /// is in scope. `(&cache).get(key)` selects the untracked trait method.
    pub fn get(&mut self, key: &K) -> Option<&Value<'a>>
    where
        K: Clone,
    {
        self.get_or_populate_tracked(key).get()
    }

    /// Check whether creation is allowed and record checks for tracked keys.
    pub fn insert(&mut self, key: &K, value: Value<'static>) -> Result<(), StoreError>
    where
        K: Clone,
    {
        let tracked = self.get_or_populate_tracked(key);

        if tracked.is_live() {
            tracked.check();
            return Err(StoreError::ValueCannotBeRecreated);
        }
        tracked.set(value)
    }

    /// Record a delta attempt, ignoring any returned error.
    pub fn add_delta(&mut self, key: &K, delta: Delta) -> Result<(), StateError>
    where
        K: Clone,
    {
        self.get_or_populate_tracked(key).add_delta(delta)
    }

    /// Return whether a live value exists locally or in the fallback.
    pub fn exists(&self, key: &K) -> bool {
        match self.cache.get(key) {
            Some(tracked) => tracked.is_live(),
            None => self
                .fallback
                .as_ref()
                .map_or(false, |fallback: &&dyn FallbackStore<K, Value<'a>>| {
                    (**fallback).contains_key(key)
                }),
        }
    }

    /// Mark the tracked value as deleted and record a write.
    pub fn delete(&mut self, key: &K) -> Result<(), StoreError>
    where
        K: Clone,
    {
        self.get_or_populate_tracked(key).delete()
    }

    pub fn views(&self) -> (Vec<(&K, &Tracked<'a>)>, Vec<(&K, &Value<'a>)>) {
        let access_records = self.cache.iter().collect();

        let transitions = self
            .cache
            .iter()
            .filter(|(_, tracked)| tracked.is_live())
            .map(|(key, tracked)| (key, tracked.value()))
            .collect();

        (access_records, transitions)
    }

    /// Get a local record, borrowing from fallback or tracking a missing value.
    fn get_or_populate_tracked(&mut self, key: &K) -> &mut Tracked<'a>
    where
        K: Clone,
    {
        let fallback = self.fallback.as_ref();

        self.cache.entry(key.clone()).or_insert_with(|| {
            fallback
                .and_then(|fallback| fallback.get(key))
                .map_or_else(Tracked::new_empty, |value| {
                    Tracked::new_borrowed(value.clone())
                })
        })
    }
}

/// A write-only store implementation for the execution cache.
/// Useful for using the execution cache as the fallback store for another execution cache.
impl<'a, K> WriteOnlyStore<K, Value<'static>> for VmCache<'a, K>
where
    K: std::hash::Hash + Eq + Clone,
{
    fn stage(&mut self, updates: Vec<(K, Value<'static>)>) -> Result<(), StoreError> {
        for (key, value) in updates {
            if !self.exists(&key) {
                self.insert(&key, value)?;
            }
        }
        Ok(())
    }

    /// Apply local deltas for the supplied keys; incoming values are unused.
    fn commit(&mut self, _: Vec<(K, Value<'static>)>) {}
}

/// VmCache as the fallback store for another VmCache,
/// allowing for a layered caching mechanism.
impl<'a, K> FallbackStore<'a, K, Value<'a>> for VmCache<'a, K>
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

#[cfg(test)]
#[path = "vm_cache_test.rs"]
mod vm_cache_test;
