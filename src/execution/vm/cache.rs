use crate::crdt::state::Delta;
use crate::crdt::state::{Status, Tracked, Value, status};
use crate::execution::error::Error;
use crate::execution::output::ExecutionOutput;
use crate::store::traits::FallbackStore;
use crate::store::traits::StoreError;
use std::borrow::Cow;
use std::collections::HashMap;

pub struct VmCache<'a, K> {
    pub(super) id: u64,
    pub(super) cache: HashMap<K, Tracked<Value<'a>, Value<'a>>>,
    pub(super) fallback: Option<&'a dyn FallbackStore<'a, K, Value<'a>>>,
}

impl<'a, K: std::hash::Hash + Eq> VmCache<'a, K> {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            cache: HashMap::new(),
            fallback: None,
        }
    }

    /// Create an empty cache backed by a read-only fallback.
    pub fn new_with_fallback(id: u64, fallback: &'a dyn FallbackStore<'a, K, Value<'a>>) -> Self {
        Self {
            id,
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
            return Err(StoreError::ValueCannotBeStripped.into());
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
                .and_then(|fallback: &&dyn FallbackStore<K, Value<'a>>| {
                    (**fallback).get_untracked(key)
                })
                .is_some_and(|value| !matches!(value, Value::None)),
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

    pub fn drain(
        &mut self,
    ) -> (
        Vec<ExecutionOutput<K, Tracked<Status, Status>>>,
        Vec<ExecutionOutput<K, Tracked<Status, Status>>>,
    )
    where
        K: Clone,
    {
        let mut access_records = Vec::with_capacity(self.cache.len());
        let mut transitions = Vec::new();

        for (key, tracked) in self.cache.drain() {
            let states = status::strip(&tracked.value);

            if !tracked.is_read_only() && !tracked.is_creation_cancelled() {
                transitions.push(ExecutionOutput {
                    id: self.id,
                    key: key.clone(),
                    gas_used: 0,
                    data: tracked.clone_with_states(states.clone()),
                });
            }

            access_records.push(ExecutionOutput {
                id: self.id,
                key,
                gas_used: 0,
                data: tracked.clone_with_states(states),
            });
        }

        (access_records, transitions)
    }

    /// Get a local record, borrowing from fallback or tracking a missing value.
    pub(super) fn get_or_populate_tracked(&mut self, key: &K) -> &mut Tracked<Value<'a>, Value<'a>>
    where
        K: Clone,
    {
        let fallback = self.fallback.as_ref();

        self.cache.entry(key.clone()).or_insert_with(|| {
            fallback
                .and_then(|fallback| fallback.get_untracked(key))
                .map_or_else(
                    || Tracked::new_owned_empty(self.id),
                    |value| Tracked::new_borrowed(value, self.id),
                )
        })
    }
}

/// VmCache as the fallback store for another VmCache,
/// allowing for a layered caching mechanism.
impl<'store, 'cache, 'value, K> FallbackStore<'store, K, Value<'value>> for VmCache<'cache, K>
where
    K: std::hash::Hash + Eq + Send + Sync,
    'cache: 'value,
{
    /// Check for a live value locally or in the fallback.
    fn contains_key(&self, key: &K) -> bool {
        self.exists(key)
    }

    /// Read without tracking; local deletions hide fallback values.
    fn get(&self, key: &K) -> Option<&Value<'value>> {
        match self.cache.get(key) {
            Some(tracked) => {
                if tracked.is_live() {
                    Some(tracked.current())
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

    fn get_untracked(&self, key: &K) -> Option<&Value<'value>> {
        match self.cache.get(key) {
            Some(tracked) => Some(tracked.current()),
            None => self
                .fallback
                .as_ref()
                .and_then(|fallback| fallback.get_untracked(key)),
        }
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
