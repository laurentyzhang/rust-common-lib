use crate::committer::{Committer, traits::TransitionWriter};
use crate::crdt::{
    Crdt,
    bytes::Bytes,
    state::{Delta, DeltaOp, Numeric, NumericError, StateError, Status, Value, status::Tag},
    uint64::U64,
};
use crate::execution::{BlockCache, BlockCacheWriter, Error, VmCache, VmCacheWriter};
use crate::store::StoreError;
use crate::store::traits::FallbackStore;
use crate::store::{CachedStoreWriter, cache::CachedStore};

const CACHE_ID: u64 = 17;

fn commit<K, V, W>(writer: &mut W, updates: Vec<(K, V)>) -> Result<(), StoreError>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
    W: TransitionWriter<K, V> + Send,
{
    let mut committer = Committer::new();
    committer.register(writer);
    committer.buffer(CACHE_ID, updates)?;
    committer.stage(vec![CACHE_ID])?;
    committer.flush()
}

fn write_cached<'a, K, V>(
    store: CachedStore<'a, K, V>,
    updates: Vec<(K, V)>,
) -> CachedStore<'a, K, V>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
    V: Clone + Send + Sync,
{
    let mut writer = CachedStoreWriter::new(store);
    commit(&mut writer, updates).unwrap();
    writer.into_inner()
}

fn write_vm<'a, K>(cache: VmCache<'a, K>, updates: Vec<(K, Value<'static>)>) -> VmCache<'a, K>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
{
    let mut writer = VmCacheWriter::new(cache);
    commit(&mut writer, updates).unwrap();
    writer.into_inner()
}

fn write_block<'a, K>(
    cache: BlockCache<'a, K>,
    updates: Vec<(K, Value<'static>)>,
) -> BlockCache<'a, K>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
{
    let mut writer = BlockCacheWriter::new(cache);
    commit(&mut writer, updates).unwrap();
    writer.into_inner()
}

fn drain_values<'a, K>(cache: &mut VmCache<'a, K>) -> Vec<(K, Value<'static>)>
where
    K: Eq + std::hash::Hash + Clone,
{
    let transitions = cache
        .cache
        .iter()
        .filter(|(_, tracked)| !tracked.is_read_only() && !tracked.is_creation_cancelled())
        .map(|(key, tracked)| (key.clone(), tracked.current().applied().into_owned()))
        .collect();

    cache.drain();
    transitions
}

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
fn deleting_missing_values_and_deleting_twice_succeed() {
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    for _ in 0..2 {
        assert_eq!(cache.delete(&7), Ok(()));
    }

    assert!(cache.insert(&7, numeric_u64(42)).is_ok());
    assert_eq!(cache.delete(&7), Ok(()));
    assert_eq!(cache.delete(&7), Ok(()));
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
fn created_values_can_be_deleted_and_created_again() {
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
    }
    assert!((&mut cache).get(&other_key).as_deref() == Some(&other_value));
}

#[test]
fn local_deletion_is_permanent_and_does_not_modify_fallback() {
    for read_first in [false, true] {
        let original = numeric_u64(17);
        let replacement = numeric_u64(42);
        let mut fallback = CachedStore::new(4, None);
        fallback = write_cached(fallback, vec![(7, original.clone())]);
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

        assert_eq!(
            cache.insert(&7, replacement),
            Err(Error::Store(StoreError::ValueCannotBeRecreated))
        );
        assert!((&cache).get(&7).is_none());
        assert!(fallback.get(&7) == Some(&original));
    }
}

#[test]
fn outer_cache_treats_inner_deletions_as_missing_records() {
    let original = numeric_u64(17);
    let replacement = numeric_u64(42);
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(7, original.clone())]);
    let mut inner = VmCache::new_with_fallback(CACHE_ID, &fallback);
    assert_eq!(inner.delete(&7), Ok(()));
    assert!((&mut inner).get(&8).is_none());
    let mut outer = VmCache::new_with_fallback(CACHE_ID, &inner);

    for key in [7, 8] {
        assert!(!outer.exists(&key));
        assert!((&outer).get(&key).is_none());
        assert!((&mut outer).get(&key).is_none());
    }

    assert!(outer.insert(&7, replacement.clone()).is_ok());
    assert!(outer.insert(&8, replacement.clone()).is_ok());
    assert!((&outer).get(&7) == Some(&replacement));
    assert!((&outer).get(&8) == Some(&replacement));
    assert!(!inner.exists(&7));
    assert!((&inner).get(&7).is_none());

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
                fallback = write_cached(fallback, vec![(7, value.clone())]);
            }
            let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
            let mut created = false;
            let mut permanently_deleted = false;
            let mut operations = sequence;
            for step in 0..5 {
                let operation = operations % 4;
                operations /= 4;
                match operation {
                    0 | 1 => {
                        let value = numeric_u64(if operation == 0 { 0 } else { u64::MAX });
                        let result = cache.insert(&7, value.clone());
                        if expected.is_some() || permanently_deleted {
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
                            created = true;
                        }
                    }
                    2 => {
                        if expected.take().is_some() {
                            if created {
                                created = false;
                            } else {
                                permanently_deleted = true;
                            }
                        }
                        assert_eq!(cache.delete(&7), Ok(()), "sequence {sequence}, step {step}");
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
fn missing_key_can_be_deleted_created_read_and_deleted_again() {
    let key = 7;
    let expected = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    // Reading a missing key returns no value and creates a tracking record.
    assert!((&mut cache).get(&key).is_none());
    assert!(cache.cache.contains_key(&key));

    // Deleting a missing value is idempotent.
    assert_eq!(cache.delete(&key), Ok(()));

    // Create a valid value and read it back.
    assert!(cache.insert(&key, expected.clone()).is_ok());
    assert!((&mut cache).get(&key).as_deref() == Some(&expected));

    // Deletion now succeeds, and subsequent reads return no value.
    assert_eq!(cache.delete(&key), Ok(()));
    assert!((&mut cache).get(&key).is_none());
}

#[test]
fn deleting_a_previously_read_missing_key_succeeds() {
    let key = 7;
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    assert!((&mut cache).get(&key).is_none());
    assert!(cache.cache.contains_key(&key));
    assert_eq!(cache.delete(&key), Ok(()));
    assert!(!cache.exists(&key));
}

#[test]
fn create_handles_existing_records_and_rejects_live_values() {
    let value = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(1, value.clone())]);
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
    fallback = write_cached(fallback, vec![(key, value.clone())]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    assert!((&cache).get(&key) == Some(&value));
    assert!(cache.cache.is_empty());

    assert!((&mut cache).get(&key).as_deref() == Some(&value));
    assert!(cache.cache.contains_key(&key));
    let tracked = cache.cache.get(&key).unwrap();
    assert!(tracked.current() == fallback.get(&key).unwrap());
}

#[test]
fn mutable_read_populates_only_the_outer_cache() {
    let key = 7;
    let value = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(key, value.clone())]);
    let inner = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let mut outer = VmCache::new_with_fallback(CACHE_ID, &inner);

    assert!((&mut outer).get(&key).as_deref() == Some(&value));
    assert!(outer.cache.contains_key(&key));
    assert!(inner.cache.is_empty());
}

#[test]
fn drain_includes_all_accesses_but_only_dirty_transitions() {
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(1, numeric_u64(10)), (2, numeric_u64(20))]);
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
    assert!(accesses.iter().all(|output| output.data.id == CACHE_ID));
    assert_eq!(transitions.len(), 1);
    assert_eq!(transitions[0].key, 2);
    assert_eq!(transitions[0].data.id, CACHE_ID);
    assert_eq!(cache.size(), 0);
}

#[test]
fn drain_clears_pending_deltas_and_subsequent_reads_reload_fallback() {
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(1, numeric_u64(10))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    cache
        .add_delta(&1, Delta::U64(DeltaOp::Add(5)))
        .expect("delta should succeed");

    let first = cache.drain().1;
    let second = cache.drain().1;

    assert_eq!(first.len(), 1);
    assert!(second.is_empty());
    assert_eq!(
        match first[0].data.current() {
            Status::Value(value) => value.as_u64(),
            Status::Tag(_) => None,
        },
        Some(10)
    );
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
    assert!(!block_cache.exists(&1));
    assert!(block_cache.get(&1).is_none());
    assert!(block_cache.drain().1.is_empty());
}

#[test]
fn staged_deletion_hides_fallback_and_cannot_be_recreated() {
    let original = numeric_u64(10);
    let replacement = numeric_u64(20);
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(1, original)]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    cache = write_vm(cache, vec![(1, Value::None)]);
    assert!(!cache.exists(&1));
    assert!(cache.get(&1).is_none());

    let mut writer = VmCacheWriter::new(cache);
    assert_eq!(
        commit(&mut writer, vec![(1, replacement)]),
        Err(StoreError::ValueCannotBeRecreated)
    );
}

#[test]
fn stage_creates_a_missing_value() {
    let fallback = CachedStore::new(4, None);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let value = numeric_u64(10);

    cache = write_vm(cache, vec![(1, value.clone())]);

    assert!(cache.exists(&1));
    assert!((&mut cache).get(&1).as_deref() == Some(&value));
}

#[test]
fn stage_adds_a_delta_to_an_existing_value() {
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(1, numeric_u64(10))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let mut update = U64::default();
    update.add_delta(&DeltaOp::Add(5)).unwrap();

    cache = write_vm(cache, vec![(1, update.into())]);

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
    fallback = write_cached(fallback, vec![(1, numeric_u64(10))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);

    cache = write_vm(cache, vec![(1, Value::None)]);

    assert!(!cache.exists(&1));
    assert!((&mut cache).get(&1).is_none());
}

#[test]
fn stage_returns_delta_errors_without_changing_the_value() {
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(1, numeric_u64(u64::MAX))]);
    let mut cache = VmCache::new_with_fallback(CACHE_ID, &fallback);
    let mut update = U64::default();
    update.add_delta(&DeltaOp::Add(1)).unwrap();

    let mut writer = VmCacheWriter::new(cache);
    let result = commit(&mut writer, vec![(1, update.into())]);
    cache = writer.into_inner();

    assert!(matches!(
        result,
        Err(StoreError::State(StateError::U64(NumericError::Overflow(
            _
        ))))
    ));
    assert_eq!(
        (&mut cache)
            .get(&1)
            .and_then(|value| value.as_ref().as_u64()),
        Some(u64::MAX)
    );
}

#[test]
fn deleted_value_is_discarded_and_rejects_deltas() {
    let mut fallback = CachedStore::new(4, None);
    fallback = write_cached(fallback, vec![(1, numeric_u64(10))]);
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
    assert_eq!(transitions[0].key, 1);
    assert!(matches!(
        transitions[0].data.current(),
        Status::Tag(Tag::Deleted)
    ));
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

    let transitions = drain_values(&mut vm_cache);
    block_cache = write_vm(block_cache, transitions);
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

    let transitions = drain_values(&mut vm_cache);
    block_cache = write_vm(block_cache, transitions);
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

    let transitions = drain_values(&mut flushed_cache);
    block_cache = write_vm(block_cache, transitions);

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

    let transitions = drain_values(&mut reread_cache);
    block_cache = write_vm(block_cache, transitions);

    let mut deletion_check_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
    assert!(!deletion_check_cache.exists(&1));
    assert!((&mut deletion_check_cache).get(&1).is_none());

    assert!(
        deletion_check_cache
            .insert(&1, Bytes::new(vec![80, 81, 82]).unwrap().into())
            .is_ok()
    );
}

#[test]
fn vm_cache_with_block_cache_fallback() {
    let mut fallback = CachedStore::<u64, Value<'_>>::new(8, None);
    fallback = write_cached(fallback, vec![(4, numeric_u64(44))]);
    let mut block_cache = BlockCache::new_with_fallback(Some(&fallback));

    let transitions = {
        let mut vm_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);

        assert!((&mut vm_cache).get(&7).is_none());
        assert!(vm_cache.cache.contains_key(&7));
        assert!(!block_cache.contains_key(&7));

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

        drain_values(&mut vm_cache)
    };
    block_cache = write_block(block_cache, transitions);
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

        drain_values(&mut vm_cache)
    };
    block_cache = write_block(block_cache, transitions);

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

        drain_values(&mut vm_cache)
    };
    block_cache = write_block(block_cache, transitions);

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

        drain_values(&mut vm_cache)
    };
    block_cache = write_block(block_cache, transitions);
    assert!(block_cache.contains_key(&1));
    assert!(matches!(block_cache.get(&1), Some(Value::None)));

    let mut final_cache = VmCache::new_with_fallback(CACHE_ID, &block_cache);
    assert!(!final_cache.exists(&1));
    assert!((&mut final_cache).get(&1).is_none());
    assert!(
        final_cache
            .insert(&1, Bytes::new(vec![80, 81, 82]).unwrap().into())
            .is_ok()
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
