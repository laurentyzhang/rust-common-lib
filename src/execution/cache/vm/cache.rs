use super::Error;
use crate::crdt::state::CrdtDelta;
use crate::crdt::state::{
    StateError, Status, Tracked, op::Operations, status::Tag, value::CrdtValue,
    value::ValueSnapshots,
};
use crate::execution::cache::Entry;
use crate::execution::output::ExecutionOutput;
use crate::store::traits::FallbackStore;
use crate::store::traits::StoreError;
use std::borrow::Cow;
use std::collections::HashMap;

pub struct VmStateCache<'a, K, T = ()> {
    pub(super) id: u64,
    pub(super) cache: HashMap<K, Tracked<Entry<'a, T>, Entry<'a, T>>>,
    pub(super) fallback: Option<&'a dyn FallbackStore<K, Entry<'a, T>>>,
}

impl<'a, K: std::hash::Hash + Eq, T: Clone> VmStateCache<'a, K, T> {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            cache: HashMap::new(),
            fallback: None,
        }
    }

    /// Create an empty cache backed by a read-only fallback.
    pub fn new_with_fallback(id: u64, fallback: &'a dyn FallbackStore<K, Entry<'a, T>>) -> Self {
        Self {
            id,
            cache: HashMap::new(),
            fallback: Some(fallback),
        }
    }

    pub fn size(&self) -> u64 {
        self.cache.len() as u64
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    /// Populate the cache if needed and record a tracked read.
    ///
    /// Use `(&mut cache).get(key)` to select this method when `FallbackStore`
    /// is in scope. `(&cache).get(key)` selects the untracked trait method.
    pub fn get(&mut self, key: &K) -> Option<Cow<'_, Entry<'a, T>>>
    where
        K: Clone,
    {
        self.try_get(key).expect("fallback store read failed")
    }

    pub fn try_get(&mut self, key: &K) -> Result<Option<Cow<'_, Entry<'a, T>>>, Error>
    where
        K: Clone,
    {
        let tracked = self.try_get_or_populate_tracked(key)?;
        let touched = tracked.has_delta();
        let Some(value) = tracked.get_entry() else {
            return Ok(None);
        };

        Ok(Some(if touched {
            Cow::Owned(value.applied())
        } else {
            Cow::Borrowed(value)
        }))
    }

    /// Check whether creation is allowed and record checks for tracked keys.
    pub fn insert(&mut self, key: &K, value: Entry<'static, T>) -> Result<(), Error>
    where
        K: Clone,
    {
        if value.is_missing() {
            return Err(StoreError::ValueCannotBeStripped.into());
        }

        let tracked = self.try_get_or_populate_tracked(key)?;

        if tracked.is_live() {
            tracked.check_entry();
            return Err(StoreError::ValueCannotBeRecreated.into());
        }
        tracked.set(value).map_err(Error::from)
    }

    /// Record a delta attempt and return any state validation error.
    pub fn add_delta(&mut self, key: &K, delta: CrdtDelta) -> Result<(), Error>
    where
        K: Clone,
    {
        self.try_get_or_populate_tracked(key)?
            .add_delta(delta)
            .map_err(Error::from)
    }

    /// Return whether a live value exists locally or in the fallback.
    pub fn exists(&self, key: &K) -> bool {
        self.try_exists(key).expect("fallback store read failed")
    }

    pub fn try_exists(&self, key: &K) -> Result<bool, Error> {
        match self.cache.get(key) {
            Some(tracked) => Ok(tracked.is_live()),
            None => Ok(self
                .fallback
                .map(|fallback| fallback.get_untracked(key))
                .transpose()?
                .flatten()
                .is_some_and(|value| !value.is_missing())),
        }
    }

    /// Mark the tracked value as deleted and record a write.
    pub fn delete(&mut self, key: &K) -> Result<(), Error>
    where
        K: Clone,
    {
        self.try_get_or_populate_tracked(key)?
            .remove()
            .map_err(Error::from)
    }

    pub fn drain(
        &mut self,
    ) -> (
        Vec<ExecutionOutput<K, Tracked<Status, Status>>>,
        Vec<ExecutionOutput<K, Tracked<(), Entry<'static, T>>>>,
    )
    where
        K: Clone,
    {
        let mut access_records = Vec::with_capacity(self.cache.len());
        let mut transitions = Vec::new();

        for (key, tracked) in self.cache.drain() {
            let states = reduce_entry_statuses(&tracked.value);
            let k = key.clone();
            access_records.push(ExecutionOutput {
                id: self.id,
                key,
                gas_used: 0,
                data: tracked.clone_with_states(states),
            });

            if !tracked.is_read_only() && !tracked.is_creation_cancelled() {
                let new_tracked = Tracked {
                    id: self.id,
                    value: ValueSnapshots {
                        original: (),
                        current: tracked.value.current.clone().into_owned(),
                    },
                    operations: tracked.operations,
                };

                transitions.push(ExecutionOutput {
                    id: self.id,
                    key: k,
                    gas_used: 0,
                    data: new_tracked,
                });
            }
        }
        (access_records, transitions)
    }

    /// Get a local record, borrowing from fallback or tracking a missing value.
    pub(super) fn try_get_or_populate_tracked(
        &mut self,
        key: &K,
    ) -> Result<&mut Tracked<Entry<'a, T>, Entry<'a, T>>, StoreError>
    where
        K: Clone,
    {
        if !self.cache.contains_key(key) {
            let tracked = match self.fallback {
                Some(fallback) => fallback.get_untracked(key)?.map_or_else(
                    || Tracked::new_entry_empty(self.id),
                    |value| Tracked::new_entry_existing(value, self.id),
                ),
                None => Tracked::new_entry_empty(self.id),
            };
            self.cache.insert(key.clone(), tracked);
        }
        Ok(self.cache.get_mut(key).expect("tracked value was inserted"))
    }
}

/// VmStateCache as the fallback store for another VmStateCache,
/// allowing for a layered caching mechanism.
impl<'cache, 'value, K, T> FallbackStore<K, Entry<'value, T>> for VmStateCache<'cache, K, T>
where
    K: std::hash::Hash + Eq + Send + Sync,
    T: Clone + Send + Sync,
    'cache: 'value,
{
    /// Check for a live value locally or in the fallback.
    fn contains_key(&self, key: &K) -> Result<bool, StoreError> {
        self.try_exists(key).map_err(|error| match error {
            Error::Store(error) => error,
            Error::State(error) => StoreError::State(error),
        })
    }

    /// Read without tracking; local deletions hide fallback values.
    fn get(&self, key: &K) -> Result<Option<Entry<'value, T>>, StoreError> {
        match self.cache.get(key) {
            Some(tracked) => {
                if tracked.is_live() {
                    Ok(Some(tracked.current().clone()))
                } else {
                    Ok(None)
                }
            }

            None => match self.fallback {
                Some(fallback) => fallback.get(key),
                None => Ok(None),
            },
        }
    }

    fn get_untracked(&self, key: &K) -> Result<Option<Entry<'value, T>>, StoreError> {
        match self.cache.get(key) {
            Some(tracked) => Ok(Some(tracked.current().clone())),
            None => match self.fallback {
                Some(fallback) => fallback.get_untracked(key),
                None => Ok(None),
            },
        }
    }
}

impl<'a, T: Clone> Tracked<Entry<'a, T>, Entry<'a, T>> {
    fn new_entry_empty(id: u64) -> Self {
        Self::new_entry_states(
            id,
            ValueSnapshots {
                original: Entry::Crdt(CrdtValue::None),
                current: Entry::Crdt(CrdtValue::None),
            },
        )
    }

    fn new_entry_existing(value: Entry<'a, T>, id: u64) -> Self {
        Self::new_entry_states(
            id,
            ValueSnapshots {
                original: value.clone(),
                current: value,
            },
        )
    }

    fn new_entry_states(id: u64, value: ValueSnapshots<Entry<'a, T>, Entry<'a, T>>) -> Self {
        Self {
            id,
            value,
            operations: Operations::default(),
        }
    }

    pub(super) fn set(&mut self, value: Entry<'a, T>) -> Result<(), StoreError> {
        if value.is_missing() {
            return Err(StoreError::SetNoneToValue);
        }
        self.operations.deltas.increment();
        if self.is_live() || self.is_deleted() {
            return Err(StoreError::ValueCannotBeRecreated);
        }
        self.value.current = value;
        Ok(())
    }

    pub(super) fn replace(&mut self, value: Entry<'a, T>) -> Result<(), StoreError> {
        if value.is_missing() {
            return Err(StoreError::SetNoneToValue);
        }
        if self.is_deleted() {
            return Err(StoreError::ValueCannotBeRecreated);
        }
        self.operations.deltas.increment();
        self.value.current = value;
        Ok(())
    }

    pub(super) fn add_delta(&mut self, delta: CrdtDelta) -> Result<(), StateError> {
        if !self.is_live() {
            return Err(StateError::CannotAddDeltaToMissingValue);
        }
        self.operations.deltas.increment();
        match &mut self.value.current {
            Entry::Crdt(value) => value.add_delta(&delta),
            Entry::Other(_) => Err(StateError::TypeMismatch),
        }
    }

    pub(super) fn remove(&mut self) -> Result<(), StoreError> {
        self.operations.removes.increment();
        self.value.current = Entry::Crdt(CrdtValue::None);
        Ok(())
    }

    fn get_entry(&mut self) -> Option<&Entry<'a, T>> {
        self.operations.reads.increment();
        self.is_live().then_some(&self.value.current)
    }

    fn check_entry(&mut self) {
        self.operations.existence_checks.increment();
    }

    fn has_delta(&self) -> bool {
        self.operations.deltas.count() > 0
    }

    fn is_live(&self) -> bool {
        !self.value.current.is_missing()
    }

    pub(super) fn is_deleted(&self) -> bool {
        self.value.current.is_missing() && !self.value.original.is_missing()
    }

    fn is_creation_cancelled(&self) -> bool {
        self.value.current.is_missing() && self.value.original.is_missing()
    }

    fn is_read_only(&self) -> bool {
        self.operations.deltas.count() == 0 && self.operations.removes.count() == 0
    }
}

fn reduce_entry_statuses<T>(
    value: &ValueSnapshots<Entry<'_, T>, Entry<'_, T>>,
) -> ValueSnapshots<Status, Status> {
    let original = if value.original.is_missing() {
        Status::Tag(Tag::Missing)
    } else {
        Status::Tag(Tag::Stripped)
    };

    let current = match &value.current {
        Entry::Crdt(CrdtValue::Numeric(value)) => {
            Status::Value(CrdtValue::Numeric(value.clone().into_owned()))
        }
        Entry::Crdt(CrdtValue::None) => {
            if original == Status::Tag(Tag::Missing) {
                Status::Tag(Tag::Missing)
            } else {
                Status::Tag(Tag::Deleted)
            }
        }
        Entry::Crdt(value) if value.has_delta() => Status::Tag(Tag::Stripped),
        Entry::Crdt(_) | Entry::Other(_) => Status::Tag(Tag::Default),
    };

    ValueSnapshots { original, current }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
