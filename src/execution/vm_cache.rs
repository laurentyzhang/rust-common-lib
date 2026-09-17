use super::Error;
use crate::crdt::state::Delta;
use crate::crdt::state::{Tracked, Value};
use crate::store::traits::FallbackStore;
use crate::store::traits::StoreError;
use crate::store::traits::WriteOnlyStore;
use std::borrow::Cow;
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
    pub fn get(&mut self, key: &K) -> Option<Cow<'_, Value<'a>>>
    where
        K: Clone,
    {
        let tracked = self.get_or_populate_tracked(key);
        let touched = tracked.has_delta();
        let value = tracked.get()?;

        Some(if touched {
            Cow::Owned(value.applied())
        } else {
            Cow::Borrowed(value)
        })
    }

    /// Check whether creation is allowed and record checks for tracked keys.
    pub fn insert(&mut self, key: &K, value: Value<'static>) -> Result<(), Error>
    where
        K: Clone,
    {
        if matches!(value, Value::None) {
            return Err(StoreError::ValueCannotBeNone.into());
        }

        let tracked = self.get_or_populate_tracked(key);

        if tracked.is_live() {
            tracked.check();
            return Err(StoreError::ValueCannotBeRecreated.into());
        }
        tracked.set(value).map_err(Error::from)
    }

    /// Record a delta attempt and return any state validation error.
    pub fn add_delta(&mut self, key: &K, delta: Delta) -> Result<(), Error>
    where
        K: Clone,
    {
        self.get_or_populate_tracked(key)
            .add_delta(delta)
            .map_err(Error::from)
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
    pub fn delete(&mut self, key: &K) -> Result<(), Error>
    where
        K: Clone,
    {
        self.get_or_populate_tracked(key)
            .delete()
            .map_err(Error::from)
    }

    pub fn drain(&self) -> (Vec<(K, Tracked<'static>)>, Vec<(K, Value<'static>)>)
    where
        K: Clone,
    {
        let access_records: Vec<(K, Tracked<'static>)> = self
            .cache
            .iter()
            .map(|(key, tracked)| ((*key).clone(), tracked.owned_clone()))
            .collect();

        let transitions: Vec<(K, Value<'static>)> = access_records
            .iter()
            .filter(|(_, tracked)| tracked.is_live())
            .map(|(key, tracked)| ((*key).clone(), tracked.value().clone()))
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
                self.get_or_populate_tracked(&key).set(value)?;
            }
        }
        Ok(())
    }

    /// Apply local deltas for the supplied keys; incoming values are unused.
    fn commit(&mut self, _: Vec<(K, Value<'static>)>) {}
}

/// VmCache as the fallback store for another VmCache,
/// allowing for a layered caching mechanism.
impl<'store, 'cache, 'value, K> FallbackStore<'store, K, Value<'value>> for VmCache<'cache, K>
where
    K: std::hash::Hash + Eq,
    'cache: 'value,
{
    /// Check for a live value locally or in the fallback.
    fn contains_key(&self, key: &K) -> bool {
        self.exists(key)
    }

    /// Read without tracking; local tombstones hide fallback values.
    fn get(&self, key: &K) -> Option<&Value<'value>> {
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
