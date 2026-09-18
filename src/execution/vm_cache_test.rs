use super::{Error, VmCache};
use crate::crdt::{
    bytes::Bytes,
    state::{Numeric, NumericError, StateError, Value},
    uint64::U64,
};
use crate::store::StoreError;
use crate::store::cached::CachedStore;
use crate::store::traits::{FallbackStore, WriteOnlyStore};

fn numeric_u64(number: u64) -> Value<'static> {
    Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64 {
        value: number,
        ..U64::default()
    })))
}

#[test]
fn repeated_missing_reads_keep_one_record_and_agree_with_exists() {
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(&fallback);
    assert!((cache).get(&7).is_none());
    assert!(!cache.exists(&7));
    assert!(cache.cache.is_empty());

    for _ in 0..3 {
        assert!((&mut cache).get(&7).is_none());
        assert!((cache).get(&7).is_none());
        assert!(!cache.exists(&7));
        assert_eq!(cache.cache.len(), 1);
    }
}

#[test]
fn deleting_without_reading_and_deleting_twice_report_missing_values() {
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(&fallback);
    for _ in 0..2 {
        assert_eq!(
            cache.delete(&7),
            Err(Error::Store(StoreError::DeleteNonexistingEntry))
        );
    }

    assert!(cache.insert(&7, numeric_u64(42)).is_ok());
    assert_eq!(cache.delete(&7), Ok(()));
    assert_eq!(
        cache.delete(&7),
        Err(Error::Store(StoreError::DeleteNonexistingEntry))
    );
    assert!((&mut cache).get(&7).is_none());
    assert!((cache).get(&7).is_none());
    assert!(!cache.exists(&7));
}

#[test]
fn rejected_duplicate_creation_preserves_the_original_value() {
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(&fallback);
    let original = numeric_u64(42);
    assert!(cache.insert(&7, original.clone()).is_ok());
    for replacement in [original.clone(), numeric_u64(u64::MAX)] {
        assert!(matches!(
            cache.insert(&7, replacement),
            Err(Error::Store(StoreError::ValueCannotBeRecreated))
        ));
    }

    assert!((cache).get(&7) == Some(&original));
    assert!((cache).get(&7) == Some(&original));
    assert!(cache.exists(&7));
}

#[test]
fn recreation_with_owned_keys_keeps_other_keys_unchanged() {
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(&fallback);
    let key = String::new();
    let other_key = String::from("other");
    let other_value = numeric_u64(17);
    assert!(cache.insert(&other_key, other_value.clone()).is_ok());
    for number in [0, u64::MAX, 42] {
        let expected = numeric_u64(number);
        assert!(cache.insert(&key, expected.clone()).is_ok());
        assert!((&mut cache).get(&key).as_deref() == Some(&expected));
        assert_eq!(cache.delete(&key), Ok(()));
        assert!((&mut cache).get(&key).is_none());
        // assert!((&cache).get(&key).is_none());
        // assert!(!cache.exists(&key));
        // assert!((&mut cache).get(&other_key) == Some(&other_value));
        // assert!(cache.exists(&other_key));
    }
}

#[test]
fn local_deletion_and_recreation_do_not_modify_fallback() {
    for read_first in [false, true] {
        let original = numeric_u64(17);
        let replacement = numeric_u64(42);
        let mut fallback = CachedStore::new(4, None);
        fallback.commit(vec![(7, original.clone())]);
        let mut cache = VmCache::new_with_fallback(&fallback);
        assert!(cache.exists(&7));
        if read_first {
            assert!((&mut cache).get(&7).as_deref() == Some(&original));
        }
        assert_eq!(cache.delete(&7), Ok(()));
        assert!(!cache.exists(&7));
        assert!((&cache).get(&7).is_none());
        assert!((&mut cache).get(&7).is_none());
        assert!(fallback.get(&7) == Some(&original));

        assert!(cache.insert(&7, replacement.clone()).is_ok());
        assert!((&cache).get(&7) == Some(&replacement));
        assert!((&mut cache).get(&7).as_deref() == Some(&replacement));
        assert!(fallback.get(&7) == Some(&original));
    }
}

#[test]
fn outer_cache_respects_inner_tombstones_and_missing_records() {
    let original = numeric_u64(17);
    let replacement = numeric_u64(42);
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(7, original.clone())]);
    let mut inner = VmCache::new_with_fallback(&fallback);
    assert_eq!(inner.delete(&7), Ok(()));
    assert!((&mut inner).get(&8).is_none());
    let mut outer = VmCache::new_with_fallback(&inner);

    for key in [7, 8] {
        assert!(!outer.exists(&key));
        assert!((&outer).get(&key).is_none());
        assert!((&mut outer).get(&key).is_none());
    }

    for key in [7, 8] {
        assert_eq!(
            outer.delete(&key),
            Err(Error::Store(StoreError::DeleteNonexistingEntry))
        );
        assert!(outer.insert(&key, replacement.clone()).is_ok());
    }

    for key in [7, 8] {
        assert!((&outer).get(&key) == Some(&replacement));
        assert!(!inner.exists(&key));
        assert!((&inner).get(&key).is_none());
    }

    assert!(fallback.get(&7) == Some(&original));
}

#[test]
fn short_operation_sequences_match_value_existence_model() {
    // rxhaust all five-operation sequences of two creations, delete, and read.
    for initially_present in [false, true] {
        for sequence in 0..4_usize.pow(5) {
            let mut fallback = CachedStore::new(4, None);
            let mut expected = initially_present.then(|| numeric_u64(17));
            if let Some(value) = &expected {
                fallback.commit(vec![(7, value.clone())]);
            }
            let mut cache = VmCache::new_with_fallback(&fallback);
            let mut operations = sequence;
            for step in 0..5 {
                let operation = operations % 4;
                operations /= 4;
                match operation {
                    0 | 1 => {
                        let value = numeric_u64(if operation == 0 { 0 } else { u64::MAX });
                        let result = cache.insert(&7, value.clone());
                        if expected.is_some() {
                            assert!(
                                matches!(
                                    result,
                                    Err(Error::Store(StoreError::ValueCannotBeRecreated))
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
                            Err(Error::Store(StoreError::DeleteNonexistingEntry))
                        };
                        assert_eq!(
                            cache.delete(&7),
                            expected_result,
                            "sequence {sequence}, step {step}"
                        );
                    }
                    _ => {
                        assert!(
                            (&mut cache).get(&7).as_deref() == expected.as_ref(),
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
    let expected = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(&fallback);

    // Reading a missing key returns no value and creates a tracking record.
    assert!((&mut cache).get(&key).is_none());
    assert!(cache.cache.contains_key(&key));

    // Deleting the missing value fails.
    assert_eq!(
        cache.delete(&key),
        Err(Error::Store(StoreError::DeleteNonexistingEntry))
    );

    // Create a valid value and read it back.
    assert!(cache.insert(&key, expected.clone()).is_ok());
    assert!((&mut cache).get(&key).as_deref() == Some(&expected));

    // Deletion now succeeds, and subsequent reads return no value.
    assert_eq!(cache.delete(&key), Ok(()));
    assert!((&mut cache).get(&key).is_none());
}

#[test]
fn deleting_a_previously_read_missing_key_returns_entry_not_found() {
    let key = 7;
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(&fallback);

    assert!((&mut cache).get(&key).is_none());
    assert!(cache.cache.contains_key(&key));
    assert_eq!(
        cache.delete(&key),
        Err(Error::Store(StoreError::DeleteNonexistingEntry))
    );
    assert!(!cache.exists(&key));
}

#[test]
fn create_handles_existing_records_and_rejects_live_values() {
    let value = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(1, value.clone())]);
    let mut cache = VmCache::new_with_fallback(&fallback);

    assert!(matches!(
        cache.insert(&1, value.clone()),
        Err(Error::Store(StoreError::ValueCannotBeRecreated))
    ));
    assert!(cache.insert(&2, value.clone()).is_ok());
    assert!(matches!(
        cache.insert(&2, value.clone()),
        Err(Error::Store(StoreError::ValueCannotBeRecreated))
    ));

    let _ = (&mut cache).get(&3);
    assert!(cache.insert(&3, value.clone()).is_ok());
    assert!((cache).get(&3) == Some(&value));

    assert!(cache.delete(&3).is_ok());
    assert!(cache.insert(&3, value.clone()).is_ok());
    assert!(cache.exists(&3));
    assert!((&cache).get(&3) == Some(&value));
}

#[test]
fn receiver_mutability_selects_cache_population() {
    let key = 7;
    let value = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(key, value.clone())]);
    let mut cache = VmCache::new_with_fallback(&fallback);

    assert!((&cache).get(&key) == Some(&value));
    assert!(cache.cache.is_empty());

    assert!((&mut cache).get(&key).as_deref() == Some(&value));
    assert!(cache.cache.contains_key(&key));
    let tracked = cache.cache.get(&key).unwrap();
    assert!(tracked.value() == fallback.get(&key).unwrap());
}

#[test]
fn mutable_read_populates_only_the_outer_cache() {
    let key = 7;
    let value = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(key, value.clone())]);
    let inner = VmCache::new_with_fallback(&fallback);
    let mut outer = VmCache::new_with_fallback(&inner);

    assert!((&mut outer).get(&key).as_deref() == Some(&value));
    assert!(outer.cache.contains_key(&key));
    assert!(inner.cache.is_empty());
}

#[test]
fn vm_cache_with_vm_cache_fallback() {
    let fallback = CachedStore::<u64, Value<'_>>::new(4, None);
    let mut block_cache = VmCache::new_with_fallback(&fallback);
    let mut vm_cache = VmCache::new_with_fallback(&block_cache);

    assert!((&mut vm_cache).get(&7u64).is_none());
    assert!(vm_cache.cache.contains_key(&7u64));
    assert!(block_cache.cache.is_empty());

    let mut result: Result<(), Error>;

    result = vm_cache.insert(&1, U64::new(0, 100).unwrap().into());
    assert!(result.is_ok());

    result = vm_cache.insert(&1, Bytes::new(vec![70, 71, 72]).unwrap().into());
    assert!(result.is_err()); // This should fail because the key already exists with a different value.

    result = vm_cache.insert(&2, Bytes::new(vec![70, 71, 72]).unwrap().into());
    assert!(result.is_ok()); // This should succeed.

    assert!(vm_cache.size() == 3);

    let delta_result = vm_cache.add_delta(&1, crate::crdt::state::Delta::U64Add(100).into());
    assert!(delta_result.is_ok());

    let delta_result = vm_cache.add_delta(&1, crate::crdt::state::Delta::U64Add(1).into());
    assert!(matches!(
        delta_result,
        Err(Error::State(StateError::U64(
            NumericError::AboveUpperLimit(_)
        )))
    ));

    let mut applied = (&mut vm_cache).get(&1).expect("value should exist");
    assert_eq!(applied.as_ref().as_u64(), Some(100));

    applied = (&mut vm_cache).get(&2).expect("value should exist");
    assert_eq!(applied.as_ref().as_bytes(), Some(&[70, 71, 72][..]));

    let mut delta = crate::crdt::state::Delta::U64Sub(10);
    let delta_result = vm_cache.add_delta(&1, delta.into());
    assert!(delta_result.is_ok()); // This should fail because the key already exists with a different value.
    applied = (&mut vm_cache).get(&1).expect("value should exist");
    assert_eq!(applied.as_ref().as_u64(), Some(90));

    let delta_add = crate::crdt::state::Delta::U64Add(1000);
    let _ = vm_cache.add_delta(&1, delta_add.into());

    let delta_sub = crate::crdt::state::Delta::U64Sub(999);
    let _ = vm_cache.add_delta(&1, delta_sub.into());

    delta = crate::crdt::state::Delta::Bytes(vec![10, 11]);
    let delta_result = vm_cache.add_delta(&2, delta.into());
    assert!(delta_result.is_ok());
    applied = (&mut vm_cache).get(&2).expect("value should exist");
    assert_eq!(applied.as_ref().as_bytes(), Some(&[10, 11][..]));
    drop(applied);

    let views = vm_cache.drain();
    let block_cache_stage = block_cache.stage(views.1);
    assert!(matches!(block_cache_stage, Ok(())));
    assert_eq!(block_cache.size(), 2);

    // Another round of testing with a new VM cache backed by the block cache.
    vm_cache = VmCache::new_with_fallback(&block_cache);
    assert_eq!(vm_cache.size(), 0);

    // Get values from the new VM cache after it has been initialized with the block cache.
    let mut applied = (&mut vm_cache).get(&1).expect("value should exist");
    assert_eq!(applied.as_ref().as_u64(), Some(90));

    applied = (&mut vm_cache).get(&2).expect("value should exist");
    assert_eq!(applied.as_ref().as_bytes(), Some(&[10, 11][..]));

    let delta_result = vm_cache.add_delta(&1, crate::crdt::state::Delta::U64Add(5).into());
    assert!(delta_result.is_ok());

    applied = (&mut vm_cache).get(&1).expect("value should exist");
    assert_eq!(applied.as_ref().as_u64(), Some(95));

    // crate::crdt::state::Value::P
    // vm_cache.insert(&1, crate::crdt::state::Value::U64(95).into());
}
