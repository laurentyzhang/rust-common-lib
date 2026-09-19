use super::{Error, VmCache};
use crate::crdt::{
    Crdt,
    bytes::Bytes,
    state::{Delta, DeltaOp, Numeric, NumericError, StateError, Value},
    uint64::U64,
};
use crate::execution::BlockCache;
use crate::store::StoreError;
use crate::store::cached::CachedStore;
use crate::store::traits::{FallbackStore, WriteOnlyStore};

const CACHE_ID: u64 = 17;

fn numeric_u64(number: u64) -> Value<'static> {
    Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64 {
        value: number,
        ..U64::default()
    })))
}

#[test]
fn repeated_missing_reads_keep_one_record_and_agree_with_exists() {
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
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
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
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
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
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
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
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
        let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
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
    let mut inner = VmCache::new_with_fallback(CACHE_ID, &fallback);
    assert_eq!(inner.delete(&7), Ok(()));
    assert!((&mut inner).get(&8).is_none());
    let mut outer = VmCache::new_with_fallback(CACHE_ID, &inner);

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
            let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
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
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

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
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

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
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

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
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

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
    let inner = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let mut outer = VmCache::new_with_fallback(CACHE_ID, &inner);

    assert!((&mut outer).get(&key).as_deref() == Some(&value));
    assert!(outer.cache.contains_key(&key));
    assert!(inner.cache.is_empty());
}

#[test]
fn drain_includes_all_accesses_but_only_dirty_transitions() {
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(1, numeric_u64(10)), (2, numeric_u64(20))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    assert_eq!(
        (&mut cache)
            .get(&1)
            .and_then(|value| value.as_ref().as_u64()),
        Some(10)
    );
    cache
        .add_delta(&2, Delta::U64(DeltaOp::Add(5)))
        .expect("delta should succeed");
    assert!((&mut cache).get(&3).is_none());

    let (accesses, transitions) = cache.drain();
    assert_eq!(accesses.len(), 3);
    assert!(accesses.iter().all(|(_, tracked)| tracked.id == CACHE_ID));
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].0, 2);
    assert_eq!(transitions[0].1.as_u64(), Some(25));
    assert_eq!(cache.size(), 0);
}

#[test]
fn drain_clears_pending_deltas_and_subsequent_reads_reload_fallback() {
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(1, numeric_u64(10))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    cache
        .add_delta(&1, Delta::U64(DeltaOp::Add(5)))
        .expect("delta should succeed");

    let first = cache.drain().1;
    let second = cache.drain().1;

    assert_eq!(first.len(), 1);
    assert!(second.is_empty());
    assert_eq!(first[0].1.as_u64(), Some(15));
    assert_eq!(
        (&mut cache)
            .get(&1)
            .and_then(|value| value.as_ref().as_u64()),
        Some(10)
    );
}

#[test]
fn create_then_delete_produces_no_transition() {
    let fallback = CachedStore::<u64, Value<'_>>::new(4, None);
    let mut block_cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    let transitions = {
        let mut vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
        vm_cache.insert(&1, numeric_u64(10)).unwrap();
        vm_cache.delete(&1).unwrap();
        vm_cache.drain().1
    };

    assert!(transitions.is_empty());
    block_cache
        .stage(transitions)
        .expect("staging no transitions should succeed");

    assert!(!block_cache.exists(&1));
    assert!(block_cache.get(&1).is_none());
    assert!(block_cache.drain().1.is_empty());
}

#[test]
fn staged_tombstone_hides_fallback_and_can_be_recreated() {
    let original = numeric_u64(10);
    let replacement = numeric_u64(20);
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(1, original)]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    cache.stage(vec![(1, Value::None)]).unwrap();
    assert!(!cache.exists(&1));
    assert!(cache.get(&1).is_none());

    cache.stage(vec![(1, replacement.clone())]).unwrap();
    assert!(cache.exists(&1));
    assert!(cache.get(&1) == Some(&replacement));
}

#[test]
fn stage_creates_a_missing_value() {
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let value = numeric_u64(10);

    cache.stage(vec![(1, value.clone())]).unwrap();

    assert!(cache.exists(&1));
    assert!((&mut cache).get(&1).as_deref() == Some(&value));
}

#[test]
fn stage_adds_a_delta_to_an_existing_value() {
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(1, numeric_u64(10))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let mut update = U64::default();
    update.add_delta(&DeltaOp::Add(5)).unwrap();

    cache.stage(vec![(1, update.into())]).unwrap();

    assert_eq!(
        (&mut cache)
            .get(&1)
            .and_then(|value| value.as_ref().as_u64()),
        Some(15)
    );
}

#[test]
fn stage_deletes_an_existing_value() {
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(1, numeric_u64(10))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    cache.stage(vec![(1, Value::None)]).unwrap();

    assert!(!cache.exists(&1));
    assert!((&mut cache).get(&1).is_none());
}

#[test]
fn stage_returns_delta_errors_without_changing_the_value() {
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(1, numeric_u64(u64::MAX))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let mut update = U64::default();
    update.add_delta(&DeltaOp::Add(1)).unwrap();

    let result = cache.stage(vec![(1, update.into())]);

    assert!(matches!(
        result,
        Err(StoreError::State(StateError::U64(
            NumericError::Overflow { .. }
        )))
    ));
    assert_eq!(
        (&mut cache)
            .get(&1)
            .and_then(|value| value.as_ref().as_u64()),
        Some(u64::MAX)
    );
}

#[test]
fn deleted_value_rejects_deltas_even_though_tombstone_retains_old_value() {
    let mut fallback = CachedStore::new(4, None);
    fallback.commit(vec![(1, numeric_u64(10))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    cache.delete(&1).unwrap();
    assert_eq!(
        cache.add_delta(&1, Delta::U64(DeltaOp::Add(5))),
        Err(Error::State(StateError::CannotAddDeltaToMissingValue))
    );
    assert!(!cache.exists(&1));
    assert!(cache.get(&1).is_none());

    let transitions = cache.drain().1;
    assert_eq!(transitions.len(), 1);
    assert!(matches!(transitions[0], (1, Value::None)));
}

#[test]
fn vm_cache_with_vm_cache_fallback() {
    let fallback = CachedStore::<u64, Value<'_>>::new(4, None);
    let mut block_cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let mut vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);

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

    let delta_result = vm_cache.add_delta(&1, Delta::U64(DeltaOp::Add(100)).into());
    assert!(delta_result.is_ok());

    let delta_result = vm_cache.add_delta(&1, Delta::U64(DeltaOp::Add(1)).into());
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

    let mut delta = Delta::U64(DeltaOp::Sub(10));
    let delta_result = vm_cache.add_delta(&1, delta.into());
    assert!(delta_result.is_ok()); // This should fail because the key already exists with a different value.
    applied = (&mut vm_cache).get(&1).expect("value should exist");
    assert_eq!(applied.as_ref().as_u64(), Some(90));

    let delta_add = Delta::U64(DeltaOp::Add(1000));
    let _ = vm_cache.add_delta(&1, delta_add.into());

    let delta_sub = Delta::U64(DeltaOp::Sub(999));
    let _ = vm_cache.add_delta(&1, delta_sub.into());

    delta = Delta::Bytes(vec![10, 11]);
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
    vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
    assert_eq!(vm_cache.size(), 0);

    // Get values from the new VM cache after it has been initialized with the block cache.
    let mut applied = (&mut vm_cache).get(&1).expect("value should exist");
    assert_eq!(applied.as_ref().as_u64(), Some(90));

    applied = (&mut vm_cache).get(&2).expect("value should exist");
    assert_eq!(applied.as_ref().as_bytes(), Some(&[10, 11][..]));

    let delta_result = vm_cache.add_delta(&1, Delta::U64(DeltaOp::Add(5)).into());
    assert!(delta_result.is_ok());

    applied = (&mut vm_cache).get(&1).expect("value should exist");
    assert_eq!(applied.as_ref().as_u64(), Some(95));

    assert_eq!(vm_cache.size(), 2);

    let mut result = vm_cache.insert(&3, crate::crdt::u64_set::U64Set::new().unwrap().into());
    assert!(result.is_ok());

    result = vm_cache.add_delta(&3, Delta::U64Set(vec![DeltaOp::Add(11)]).into());
    assert_eq!(matches!(result, Ok(())), true);

    result = vm_cache.add_delta(&3, Delta::U64Set(vec![DeltaOp::Add(21)]).into());
    assert_eq!(matches!(result, Ok(())), true);

    result = vm_cache.add_delta(&3, Delta::U64Set(vec![DeltaOp::Add(31)]).into());
    assert_eq!(matches!(result, Ok(())), true);
    assert_eq!(vm_cache.size(), 3);

    {
        let value = (&mut vm_cache).get(&3).expect("value should exist");
        let entries = value
            .as_ref()
            .as_u64_set()
            .expect("value should be a U64Set");

        assert_eq!(entries.get(&11), Some(&11));
        assert_eq!(entries.get(&21), Some(&21));
        assert_eq!(entries.get(&31), Some(&31));
    }

    let (_, transitions) = vm_cache.drain();
    block_cache
        .stage(transitions)
        .expect("flush VM cache to block cache");
    assert_eq!(block_cache.size(), 3);

    let mut flushed_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
    {
        let value = (&mut flushed_cache)
            .get(&3)
            .expect("flushed value should exist");
        let entries = value
            .as_ref()
            .as_u64_set()
            .expect("flushed value should be a U64Set");

        assert_eq!(entries.get(&11), Some(&11));
        assert_eq!(entries.get(&21), Some(&21));
        assert_eq!(entries.get(&31), Some(&31));
    }

    flushed_cache
        .add_delta(&3, Delta::U64Set(vec![DeltaOp::Add(41), DeltaOp::Sub(21)]))
        .expect("update flushed U64Set");

    let (_, transitions) = flushed_cache.drain();
    block_cache
        .stage(transitions)
        .expect("flush updated VM cache to block cache");

    let mut reread_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
    {
        let value = (&mut reread_cache)
            .get(&3)
            .expect("updated flushed value should exist");
        let entries = value
            .as_ref()
            .as_u64_set()
            .expect("updated flushed value should be a U64Set");

        assert_eq!(entries.get(&11), Some(&11));
        assert_eq!(entries.get(&21), None);
        assert_eq!(entries.get(&31), Some(&31));
        assert_eq!(entries.get(&41), Some(&41));
    }

    assert!(reread_cache.exists(&1));
    reread_cache.delete(&1).expect("delete key 1");
    assert!(!reread_cache.exists(&1));

    let (_, transitions) = reread_cache.drain();
    block_cache
        .stage(transitions)
        .expect("flush deleted key to block cache");

    let mut deletion_check_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
    assert!(!deletion_check_cache.exists(&1));
    assert!((&mut deletion_check_cache).get(&1).is_none());

    deletion_check_cache
        .insert(&1, Bytes::new(vec![80, 81, 82]).unwrap().into())
        .expect("recreate key 1 as bytes");
    assert!(deletion_check_cache.exists(&1));

    let (_, transitions) = deletion_check_cache.drain();
    block_cache
        .stage(transitions)
        .expect("flush recreated key to block cache");

    let mut recreation_check_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
    assert!(recreation_check_cache.exists(&1));
    let value = (&mut recreation_check_cache)
        .get(&1)
        .expect("recreated value should exist");
    assert_eq!(value.as_ref().as_bytes(), Some(&[80, 81, 82][..]));
}

#[test]
fn vm_cache_with_block_cache_fallback() {
    let mut fallback = CachedStore::<u64, Value<'_>>::new(8, None);
    fallback.commit(vec![(4, numeric_u64(44))]);
    let mut block_cache = BlockCache::new_with_fallback(Some(&fallback));

    let transitions = {
        let mut vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);

        assert!((&mut vm_cache).get(&7).is_none());
        assert!(vm_cache.cache.contains_key(&7));
        assert!(!block_cache.cache.contains_key(&7));

        let fallback_value = (&mut vm_cache)
            .get(&4)
            .expect("fallback value should exist");
        assert_eq!(fallback_value.as_ref().as_u64(), Some(44));
        assert!(!block_cache.cache.contains_key(&4));

        vm_cache
            .insert(&1, U64::new(0, 100).unwrap().into())
            .unwrap();
        assert!(matches!(
            vm_cache.insert(&1, Bytes::new(vec![1]).unwrap().into()),
            Err(Error::Store(StoreError::ValueCannotBeRecreated))
        ));
        vm_cache
            .insert(&2, Bytes::new(vec![70, 71, 72]).unwrap().into())
            .unwrap();

        vm_cache
            .add_delta(&1, Delta::U64(DeltaOp::Add(100)))
            .unwrap();
        assert!(matches!(
            vm_cache.add_delta(&1, Delta::U64(DeltaOp::Add(1))),
            Err(Error::State(StateError::U64(
                NumericError::AboveUpperLimit(_)
            )))
        ));
        vm_cache
            .add_delta(&1, Delta::U64(DeltaOp::Sub(10)))
            .unwrap();
        vm_cache.add_delta(&2, Delta::Bytes(vec![10, 11])).unwrap();

        assert_eq!(
            (&mut vm_cache)
                .get(&1)
                .expect("numeric value should exist")
                .as_ref()
                .as_u64(),
            Some(90)
        );
        assert_eq!(
            (&mut vm_cache)
                .get(&2)
                .expect("byte value should exist")
                .as_ref()
                .as_bytes(),
            Some(&[10, 11][..])
        );

        vm_cache.drain().1
    };
    block_cache
        .stage(transitions)
        .expect("flush initial VM cache to block cache");
    assert!(block_cache.contains_key(&1));
    assert!(block_cache.contains_key(&2));

    let transitions = {
        let mut vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
        assert_eq!(
            (&mut vm_cache)
                .get(&1)
                .expect("flushed numeric value should exist")
                .as_ref()
                .as_u64(),
            Some(90)
        );
        assert_eq!(
            (&mut vm_cache)
                .get(&2)
                .expect("flushed byte value should exist")
                .as_ref()
                .as_bytes(),
            Some(&[10, 11][..])
        );

        vm_cache.add_delta(&1, Delta::U64(DeltaOp::Add(5))).unwrap();
        vm_cache
            .insert(&3, crate::crdt::u64_set::U64Set::new().unwrap().into())
            .unwrap();
        vm_cache
            .add_delta(
                &3,
                Delta::U64Set(vec![DeltaOp::Add(11), DeltaOp::Add(21), DeltaOp::Add(31)]),
            )
            .unwrap();

        vm_cache.drain().1
    };
    block_cache
        .stage(transitions)
        .expect("flush numeric and set updates to block cache");

    let transitions = {
        let mut vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
        assert_eq!(
            (&mut vm_cache)
                .get(&1)
                .expect("updated numeric value should exist")
                .as_ref()
                .as_u64(),
            Some(95)
        );

        {
            let value = (&mut vm_cache).get(&3).expect("set should exist");
            let entries = value.as_ref().as_u64_set().expect("value should be a set");
            assert_eq!(entries.get(&11), Some(&11));
            assert_eq!(entries.get(&21), Some(&21));
            assert_eq!(entries.get(&31), Some(&31));
        }

        vm_cache
            .add_delta(&3, Delta::U64Set(vec![DeltaOp::Add(41), DeltaOp::Sub(21)]))
            .unwrap();

        vm_cache.drain().1
    };
    block_cache
        .stage(transitions)
        .expect("flush updated set to block cache");

    let transitions = {
        let mut vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
        {
            let value = (&mut vm_cache).get(&3).expect("updated set should exist");
            let entries = value.as_ref().as_u64_set().expect("value should be a set");
            assert_eq!(entries.get(&11), Some(&11));
            assert_eq!(entries.get(&21), None);
            assert_eq!(entries.get(&31), Some(&31));
            assert_eq!(entries.get(&41), Some(&41));
        }

        assert!(vm_cache.exists(&1));
        vm_cache.delete(&1).expect("delete key 1");
        assert!(!vm_cache.exists(&1));

        vm_cache.drain().1
    };
    block_cache
        .stage(transitions)
        .expect("flush deletion to block cache");
    assert!(!block_cache.contains_key(&1));
    assert!(block_cache.get(&1).is_none());

    let transitions = {
        let mut vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
        assert!(!vm_cache.exists(&1));
        assert!((&mut vm_cache).get(&1).is_none());

        vm_cache
            .insert(&1, Bytes::new(vec![80, 81, 82]).unwrap().into())
            .expect("recreate key 1 as bytes");

        vm_cache.drain().1
    };
    block_cache
        .stage(transitions)
        .expect("flush recreated key to block cache");

    let mut final_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
    assert_eq!(
        (&mut final_cache)
            .get(&1)
            .expect("recreated key should exist")
            .as_ref()
            .as_bytes(),
        Some(&[80, 81, 82][..])
    );
    let value = (&mut final_cache)
        .get(&3)
        .expect("set should remain present");
    let entries = value.as_ref().as_u64_set().expect("value should be a set");
    assert_eq!(entries.get(&11), Some(&11));
    assert_eq!(entries.get(&21), None);
    assert_eq!(entries.get(&31), Some(&31));
    assert_eq!(entries.get(&41), Some(&41));
}
