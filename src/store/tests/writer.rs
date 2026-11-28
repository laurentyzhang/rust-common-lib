use std::collections::HashMap;

use super::*;
use crate::store::{PhysicalStore, cache::PhysicalCachedStore};

struct MapStore(HashMap<u64, u64>);

impl PhysicalStore<u64, u64> for MapStore {
    fn get(&self, key: &u64) -> Result<Option<u64>, StoreError> {
        Ok(self.0.get(key).copied())
    }
}

impl MapStore {
    fn flush(&mut self, updates: Vec<(u64, u64)>) -> Result<(), StoreError> {
        self.0.extend(updates);
        Ok(())
    }
}

#[test]
fn consume_commit_to_backend_and_cache_then_rebuild() {
    let mut backend = MapStore(HashMap::from([(1, 10)]));

    let store = PhysicalCachedStore::<_, _>::new(
        8,
        quick_cache::UnitWeighter,
        Default::default(),
        Some(&backend),
    );
    assert_eq!(store.get_with(&1, |value| *value), Ok(Some(10)));
    let mut cache_writer = CachedStoreWriter::new(store.into_cache());

    let updates = vec![(1, 11), (2, 20)];
    backend.flush(updates.clone()).unwrap();
    cache_writer.flush(updates).unwrap();

    let store = PhysicalCachedStore::from_cache(cache_writer.into_inner(), Some(&backend));
    assert_eq!(store.get_with(&1, |value| *value), Ok(Some(11)));
    assert_eq!(store.get_with(&2, |value| *value), Ok(Some(20)));
    assert_eq!(backend.0.get(&1), Some(&11));
    assert_eq!(backend.0.get(&2), Some(&20));
}
