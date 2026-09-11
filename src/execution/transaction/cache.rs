use crate::crdt::state::{Delta, Error};
use crate::crdt::state::{Tracked, Value};
use crate::store::traits::ReadOnlyStore;
use std::collections::HashMap;

pub struct ExecutionCache<'a, K> {
    pub(super) cache: HashMap<K, Tracked<'a>>,
    pub(super) fallback: Option<&'a dyn ReadOnlyStore<'a, K, Value<'a>>>,
}

impl<'a, K: std::hash::Hash + Eq> ExecutionCache<'a, K> {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            fallback: None,
        }
    }

    /// Create an empty cache backed by a read-only fallback.
    pub fn new_with_fallback(fallback: &'a dyn ReadOnlyStore<'a, K, Value<'a>>) -> Self {
        Self {
            cache: HashMap::new(),
            fallback: Some(fallback),
        }
    }

    /// Populate the cache if needed and record a tracked read.
    ///
    /// Use `(&mut cache).get(key)` to select this method when `ReadOnlyStore`
    /// is in scope. `(&cache).get(key)` selects the untracked trait method.
    pub fn get(&mut self, key: &K) -> Option<&Value<'a>>
    where
        K: Clone,
    {
        self.get_or_populate_tracked(key.clone()).read()
    }

    /// Check whether creation is allowed and record checks for tracked keys.
    pub fn create(&mut self, key: K, value: Value<'a>) -> Result<(), crate::store::traits::Error>
    where
        K: Clone,
    {
        let tracked = self.get_or_populate_tracked(key.clone());
        if !tracked.is_live() {
            tracked.write(value);
            return Ok(());
        }
        return Err(crate::store::traits::Error::ValueAlreadyExists);
    }

    /// Record a delta attempt, ignoring any returned error.
    pub fn add_delta(&mut self, key: K, delta: Delta) -> Result<(), Error> {
        self.get_or_populate_tracked(key).add_delta(delta)
    }

    /// Return whether a live value exists locally or in the fallback.
    pub fn exists(&self, key: &K) -> bool {
        match self.cache.get(key) {
            Some(tracked) => tracked.is_live(),
            None => self.has_in_fallback(key),
        }
    }

    /// Mark the tracked value as deleted and record a write.
    pub fn delete(&mut self, key: K) -> Result<(), Error> {
        self.get_or_populate_tracked(key).delete()
    }

    /// Check if the key exists in the fallback store.
    pub(super) fn has_in_fallback(&self, key: &K) -> bool {
        self.fallback
            .as_ref()
            .map_or(false, |fallback: &&dyn ReadOnlyStore<'a, K, Value<'a>>| {
                (**fallback).contains_key(key)
            })
    }

    /// Get a mutable reference to a tracked value if it exists locally.
    fn get_tracked_mut(&mut self, key: &K) -> Option<&mut Tracked<'a>> {
        self.cache.get_mut(key)
    }

    /// Get a local record, borrowing from fallback or tracking a missing value.
    fn get_or_populate_tracked(&mut self, key: K) -> &mut Tracked<'a> {
        if self.cache.contains_key(&key) {
            return self
                .get_tracked_mut(&key)
                .expect("tracked entry must exist");
        }

        let tracked = self
            .fallback
            .as_ref()
            .and_then(|fallback| fallback.get(&key))
            .map(|value| Tracked::new_borrowed(value.clone()))
            .unwrap_or_else(Tracked::new);

        self.cache.entry(key).or_insert(tracked)
    }
}

#[cfg(test)]
mod tests {
    use super::ExecutionCache;
    use crate::crdt::{state::Value, uint64::U64};
    use crate::store::cached::CachedStore;
    use crate::store::traits::{ReadOnlyStore, WriteOnlyStore};

    fn numeric_value(number: u64) -> Value<'static> {
        Value::U64(std::borrow::Cow::Owned(U64 {
            value: Some(number),
            ..U64::default()
        }))
    }

    #[test]
    fn repeated_missing_reads_keep_one_record_and_agree_with_exists() {
        let fallback = CachedStore::new(4, None);
        let mut cache = ExecutionCache::new_with_fallback(&fallback);
        assert!((&cache).get(&7).is_none());
        assert!(!cache.exists(&7));
        assert!(cache.cache.is_empty());

        for _ in 0..3 {
            assert!((&mut cache).get(&7).is_none());
            assert!((&cache).get(&7).is_none());
            assert!(!cache.exists(&7));
            assert_eq!(cache.cache.len(), 1);
        }
    }

    #[test]
    fn deleting_without_reading_and_deleting_twice_report_missing_values() {
        let fallback = CachedStore::new(4, None);
        let mut cache = ExecutionCache::new_with_fallback(&fallback);
        for _ in 0..2 {
            assert_eq!(
                cache.delete(7),
                Err(crate::crdt::state::Error::EntryNotFound)
            );
        }
        assert!(cache.create(7, numeric_value(42)).is_ok());
        assert_eq!(cache.delete(7), Ok(()));
        assert_eq!(
            cache.delete(7),
            Err(crate::crdt::state::Error::EntryNotFound)
        );
        assert!((&mut cache).get(&7).is_none());
        assert!((&cache).get(&7).is_none());
        assert!(!cache.exists(&7));
    }

    #[test]
    fn rejected_duplicate_creation_preserves_the_original_value() {
        let fallback = CachedStore::new(4, None);
        let mut cache = ExecutionCache::new_with_fallback(&fallback);
        let original = numeric_value(42);
        assert!(cache.create(7, original.clone()).is_ok());
        for replacement in [original.clone(), numeric_value(u64::MAX)] {
            assert!(matches!(
                cache.create(7, replacement),
                Err(crate::store::traits::Error::ValueAlreadyExists)
            ));
            assert!((&mut cache).get(&7) == Some(&original));
            assert!((&cache).get(&7) == Some(&original));
            assert!(cache.exists(&7));
        }
    }

    #[test]
    fn recreation_with_owned_keys_keeps_other_keys_unchanged() {
        let fallback = CachedStore::new(4, None);
        let mut cache = ExecutionCache::new_with_fallback(&fallback);
        let key = String::new();
        let other_key = String::from("other");
        let other_value = numeric_value(17);
        assert!(cache.create(other_key.clone(), other_value.clone()).is_ok());
        for number in [0, u64::MAX, 42] {
            let expected = numeric_value(number);
            assert!(cache.create(key.clone(), expected.clone()).is_ok());
            assert!((&mut cache).get(&key) == Some(&expected));
            assert_eq!(cache.delete(key.clone()), Ok(()));
            assert!((&mut cache).get(&key).is_none());
            assert!((&cache).get(&key).is_none());
            assert!(!cache.exists(&key));
            assert!((&mut cache).get(&other_key) == Some(&other_value));
            assert!(cache.exists(&other_key));
        }
    }

    #[test]
    fn local_deletion_and_recreation_do_not_modify_fallback() {
        for read_first in [false, true] {
            let original = numeric_value(17);
            let replacement = numeric_value(42);
            let mut fallback = CachedStore::new(4, None);
            fallback.commit_batch(vec![(7, original.clone())]);
            let mut cache = ExecutionCache::new_with_fallback(&fallback);
            assert!(cache.exists(&7));
            if read_first {
                assert!((&mut cache).get(&7) == Some(&original));
            }
            assert_eq!(cache.delete(7), Ok(()));
            assert!(!cache.exists(&7));
            assert!((&cache).get(&7).is_none());
            assert!((&mut cache).get(&7).is_none());
            assert!(fallback.get(&7) == Some(&original));

            assert!(cache.create(7, replacement.clone()).is_ok());
            assert!((&cache).get(&7) == Some(&replacement));
            assert!((&mut cache).get(&7) == Some(&replacement));
            assert!(fallback.get(&7) == Some(&original));
        }
    }

    #[test]
    fn outer_cache_respects_inner_tombstones_and_missing_records() {
        let original = numeric_value(17);
        let replacement = numeric_value(42);
        let mut fallback = CachedStore::new(4, None);
        fallback.commit_batch(vec![(7, original.clone())]);
        let mut inner = ExecutionCache::new_with_fallback(&fallback);
        assert_eq!(inner.delete(7), Ok(()));
        assert!((&mut inner).get(&8).is_none());
        let mut outer = ExecutionCache::new_with_fallback(&inner);

        for key in [7, 8] {
            assert!(!outer.exists(&key));
            assert!((&outer).get(&key).is_none());
            assert!((&mut outer).get(&key).is_none());
            assert_eq!(
                outer.delete(key),
                Err(crate::crdt::state::Error::EntryNotFound)
            );
            assert!(outer.create(key, replacement.clone()).is_ok());
            assert!((&mut outer).get(&key) == Some(&replacement));
            assert!(!inner.exists(&key));
            assert!((&inner).get(&key).is_none());
        }
        assert!(fallback.get(&7) == Some(&original));
    }

    #[test]
    fn short_operation_sequences_match_value_existence_model() {
        // Exhaust all five-operation sequences of two creations, delete, and read.
        for initially_present in [false, true] {
            for sequence in 0..4_usize.pow(5) {
                let mut fallback = CachedStore::new(4, None);
                let mut expected = initially_present.then(|| numeric_value(17));
                if let Some(value) = &expected {
                    fallback.commit_batch(vec![(7, value.clone())]);
                }
                let mut cache = ExecutionCache::new_with_fallback(&fallback);
                let mut operations = sequence;
                for step in 0..5 {
                    let operation = operations % 4;
                    operations /= 4;
                    match operation {
                        0 | 1 => {
                            let value = numeric_value(if operation == 0 { 0 } else { u64::MAX });
                            let result = cache.create(7, value.clone());
                            if expected.is_some() {
                                assert!(
                                    matches!(
                                        result,
                                        Err(crate::store::traits::Error::ValueAlreadyExists)
                                    ),
                                    "sequence {sequence}, step {step}"
                                );
                            } else {
                                assert!(result.is_ok(), "sequence {sequence}, step {step}");
                                expected = Some(value);
                            }
                        }
                        2 => {
                            let expected_result = if expected.take().is_some() {
                                Ok(())
                            } else {
                                Err(crate::crdt::state::Error::EntryNotFound)
                            };
                            assert_eq!(
                                cache.delete(7),
                                expected_result,
                                "sequence {sequence}, step {step}"
                            );
                        }
                        _ => {
                            assert!(
                                (&mut cache).get(&7) == expected.as_ref(),
                                "sequence {sequence}, step {step}"
                            );
                        }
                    }
                    assert_eq!(
                        cache.exists(&7),
                        expected.is_some(),
                        "sequence {sequence}, step {step}"
                    );
                    assert!(
                        (&cache).get(&7) == expected.as_ref(),
                        "sequence {sequence}, step {step}"
                    );
                }
            }
        }
    }

    #[test]
    fn missing_key_can_be_created_read_and_deleted_after_failed_delete() {
        let key = 7;
        let expected = Value::U64(std::borrow::Cow::Owned(U64::default()));
        let fallback = CachedStore::new(4, None);
        let mut cache = ExecutionCache::new_with_fallback(&fallback);

        // Reading a missing key returns no value and creates a tracking record.
        assert!((&mut cache).get(&key).is_none());
        assert!(cache.cache.contains_key(&key));

        // Deleting the missing value fails.
        assert_eq!(
            cache.delete(key),
            Err(crate::crdt::state::Error::EntryNotFound)
        );

        // Create a valid value and read it back.
        assert!(cache.create(key, expected.clone()).is_ok());
        assert!((&mut cache).get(&key) == Some(&expected));

        // Deletion now succeeds, and subsequent reads return no value.
        assert_eq!(cache.delete(key), Ok(()));
        assert!((&mut cache).get(&key).is_none());
    }

    #[test]
    fn deleting_a_previously_read_missing_key_returns_entry_not_found() {
        let key = 7;
        let fallback = CachedStore::new(4, None);
        let mut cache = ExecutionCache::new_with_fallback(&fallback);

        assert!((&mut cache).get(&key).is_none());
        assert!(cache.cache.contains_key(&key));
        assert_eq!(
            cache.delete(key),
            Err(crate::crdt::state::Error::EntryNotFound)
        );
        assert!(!cache.exists(&key));
    }

    #[test]
    fn create_handles_existing_records_and_rejects_live_values() {
        let value = Value::U64(std::borrow::Cow::Owned(U64::default()));
        let mut fallback = CachedStore::new(4, None);
        fallback.commit_batch(vec![(1, value.clone())]);
        let mut cache = ExecutionCache::new_with_fallback(&fallback);

        assert!(matches!(
            cache.create(1, value.clone()),
            Err(crate::store::traits::Error::ValueAlreadyExists)
        ));
        assert!(cache.create(2, value.clone()).is_ok());
        assert!(matches!(
            cache.create(2, value.clone()),
            Err(crate::store::traits::Error::ValueAlreadyExists)
        ));

        let _ = (&mut cache).get(&3);
        assert!(cache.create(3, value.clone()).is_ok());
        assert!((&cache).get(&3) == Some(&value));

        assert!(cache.delete(3).is_ok());
        assert!(cache.create(3, value.clone()).is_ok());
        assert!(cache.exists(&3));
        assert!((&cache).get(&3) == Some(&value));
    }

    #[test]
    fn receiver_mutability_selects_cache_population() {
        let key = 7;
        let value = Value::U64(std::borrow::Cow::Owned(U64::default()));
        let mut fallback = CachedStore::new(4, None);
        fallback.commit_batch(vec![(key, value.clone())]);
        let mut cache = ExecutionCache::new_with_fallback(&fallback);

        assert!((&cache).get(&key) == Some(&value));
        assert!(cache.cache.is_empty());

        assert!((&mut cache).get(&key) == Some(&value));
        assert!(cache.cache.contains_key(&key));
        let tracked = cache.cache.get(&key).unwrap();
        assert!(tracked.value() == fallback.get(&key).unwrap());
    }

    #[test]
    fn mutable_read_populates_only_the_outer_cache() {
        let key = 7;
        let value = Value::U64(std::borrow::Cow::Owned(U64::default()));
        let mut fallback = CachedStore::new(4, None);
        fallback.commit_batch(vec![(key, value.clone())]);
        let inner = ExecutionCache::new_with_fallback(&fallback);
        let mut outer = ExecutionCache::new_with_fallback(&inner);

        assert!((&mut outer).get(&key) == Some(&value));
        assert!(outer.cache.contains_key(&key));
        assert!(inner.cache.is_empty());
    }

    #[test]
    fn execution_cache_with_execution_cache_fallback() {
        let fallback = CachedStore::<u64, Value<'_>>::new(4, None);
        let shared_cache = ExecutionCache::new_with_fallback(&fallback);
        let mut execution_cache = ExecutionCache::new_with_fallback(&shared_cache);

        assert!((&mut execution_cache).get(&7u64).is_none());
        assert!(execution_cache.cache.contains_key(&7u64));
        assert!(shared_cache.cache.is_empty());
    }
}
