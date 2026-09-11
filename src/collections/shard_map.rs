use rayon::prelude::*;
use std::{
    collections::HashMap,
    hash::{BuildHasherDefault, Hash, Hasher},
};

pub struct ShardMap<K, V> {
    maps: Vec<HashMap<K, V, SumBuildHasher>>,
}

impl<K: Hash + Eq, V> ShardMap<K, V> {
    pub fn new(num_shards: usize) -> Self {
        assert!(
            num_shards.is_power_of_two() && num_shards <= 256,
            "ShardMap shard count must be a non-zero power of two no greater than 256"
        );

        let mut maps = Vec::with_capacity(num_shards);
        for _ in 0..num_shards {
            maps.push(HashMap::with_hasher(SumBuildHasher::default()));
        }
        ShardMap { maps }
    }

    pub fn contains_key(&self, key: &K) -> bool {
        let shard_index = self.shard_index(key);
        if let Some(map) = self.maps.get(shard_index) {
            map.contains_key(key)
        } else {
            false
        }
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        let shard_index = self.shard_index(key);
        if let Some(map) = self.maps.get(shard_index) {
            map.get(key)
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let shard_index = self.shard_index(key);
        if let Some(map) = self.maps.get_mut(shard_index) {
            map.get_mut(key)
        } else {
            None
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let shard_index = self.shard_index(&key);
        if let Some(map) = self.maps.get_mut(shard_index) {
            map.insert(key, value)
        } else {
            None
        }
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        let shard_index = self.shard_index(key);
        if let Some(map) = self.maps.get_mut(shard_index) {
            map.remove(key)
        } else {
            None
        }
    }

    pub fn apply_batch(&mut self, updates: Vec<(K, V)>)
    where
        K: Send,
        V: Send,
    {
        let num_shards = self.maps.len();

        // First pass: count per shard so each bucket is allocated at its exact
        // final size and never reallocates while moving elements in.
        let mut counts = vec![0usize; num_shards];
        for (key, _) in &updates {
            counts[self.shard_index(key)] += 1;
        }

        let mut shard_updates: Vec<Vec<(K, V)>> =
            counts.into_iter().map(Vec::with_capacity).collect();

        // Second pass: move (not clone) each pair into its shard's bucket.
        for (key, value) in updates {
            let shard_index = self.shard_index(&key);
            shard_updates[shard_index].push((key, value));
        }

        self.maps
            .par_iter_mut()
            .zip(shard_updates.into_par_iter())
            .for_each(|(map, bucket)| {
                for (key, value) in bucket {
                    map.insert(key, value);
                }
            });
    }

    fn shard_index(&self, key: &K) -> usize {
        let mut hasher = SumHasher::default();
        key.hash(&mut hasher);
        (hasher.finish() % self.maps.len() as u64) as usize
    }
}

// Sum and modulo hasher for sharding.
#[derive(Default)]
struct SumHasher {
    sum: u64,
}

impl Hasher for SumHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.sum = self.sum.wrapping_add(byte as u64);
        }
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.sum
    }
}

type SumBuildHasher = BuildHasherDefault<SumHasher>;

// Tests for the ShardMap implementation.
#[cfg(test)]
mod tests {
    use super::ShardMap;

    #[test]
    fn apply_batch_updates_values_in_shards() {
        let mut map = ShardMap::<u64, u64>::new(4);
        let updates = vec![(1, 10), (2, 20), (1, 11), (3, 30), (4, 40), (5, 50)];

        map.apply_batch(updates);

        assert_eq!(map.get(&1), Some(&11));
        assert_eq!(map.get(&2), Some(&20));
        assert_eq!(map.get(&3), Some(&30));
        assert_eq!(map.get(&4), Some(&40));
        assert_eq!(map.get(&5), Some(&50));
        assert_eq!(map.contains_key(&6), false);
    }
}
