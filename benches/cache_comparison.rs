use std::{
    collections::{HashMap, VecDeque, hash_map::Entry},
    hint::black_box,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use foyer_memory::{Cache as FoyerCache, CacheBuilder as FoyerBuilder, S3FifoConfig};
use moka::sync::Cache as MokaCache;
use quick_cache::{Weighter, sync::Cache as QuickCache};
use rayon::prelude::*;

const ITEMS: usize = 32_768;
const READS: usize = 1_000_000;
const VALUE_BYTES: usize = 256;
const SHARDS: usize = 8;
const CAPACITY_BYTES: usize = ITEMS * VALUE_BYTES;

#[derive(Clone)]
struct Blob {
    bytes: Box<[u8]>,
}

impl Blob {
    fn new(seed: u64) -> Self {
        let mut bytes = vec![0; VALUE_BYTES].into_boxed_slice();
        bytes[0] = seed as u8;
        Self { bytes }
    }
}

type SharedBlob = Arc<Blob>;

#[derive(Clone)]
struct BlobWeighter;

impl Weighter<u64, SharedBlob> for BlobWeighter {
    fn weight(&self, _key: &u64, value: &SharedBlob) -> u64 {
        value.bytes.len() as u64
    }
}

struct ClockEntry {
    value: SharedBlob,
    referenced: AtomicBool,
}

struct ClockShard {
    entries: HashMap<u64, ClockEntry>,
    queue: VecDeque<u64>,
    weight: usize,
}

struct ClockCache {
    shards: Vec<ClockShard>,
    shard_capacity: usize,
}

impl ClockCache {
    fn new(capacity: usize) -> Self {
        Self {
            shards: (0..SHARDS)
                .map(|_| ClockShard {
                    entries: HashMap::with_capacity(ITEMS / SHARDS),
                    queue: VecDeque::with_capacity(ITEMS / SHARDS),
                    weight: 0,
                })
                .collect(),
            shard_capacity: capacity / SHARDS,
        }
    }

    fn insert(&mut self, key: u64, value: SharedBlob) {
        let shard = &mut self.shards[key as usize & (SHARDS - 1)];
        let weight = value.bytes.len();

        if let Some(entry) = shard.entries.get_mut(&key) {
            shard.weight = shard.weight - entry.value.bytes.len() + weight;
            entry.value = value;
            entry.referenced.store(true, Ordering::Relaxed);
        } else {
            shard.weight += weight;
            shard.queue.push_back(key);
            shard.entries.insert(
                key,
                ClockEntry {
                    value,
                    referenced: AtomicBool::new(false),
                },
            );
        }

        while shard.weight > self.shard_capacity {
            let Some(candidate) = shard.queue.pop_front() else {
                break;
            };
            let Some(entry) = shard.entries.get(&candidate) else {
                continue;
            };
            if entry.referenced.swap(false, Ordering::Relaxed) {
                shard.queue.push_back(candidate);
                continue;
            }
            if let Some(removed) = shard.entries.remove(&candidate) {
                shard.weight -= removed.value.bytes.len();
            }
        }
    }

    fn get(&self, key: &u64) -> Option<&SharedBlob> {
        let entry = self.shards[*key as usize & (SHARDS - 1)].entries.get(key)?;
        entry.referenced.store(true, Ordering::Relaxed);
        Some(&entry.value)
    }
}

fn value(key: u64) -> SharedBlob {
    Arc::new(Blob::new(key))
}

fn read_key(index: usize) -> u64 {
    (index as u64).wrapping_mul(11_400_714_819_323_198_485) % ITEMS as u64
}

fn measure_reads<F>(name: &str, get: F)
where
    F: Fn(&u64) -> Option<u8> + Sync,
{
    for index in 0..ITEMS {
        black_box(get(&read_key(index)));
    }

    let start = Instant::now();
    let mut checksum = 0u64;
    for index in 0..READS {
        checksum = checksum.wrapping_add(get(&read_key(index)).unwrap_or_default() as u64);
    }
    let sequential = start.elapsed();

    let start = Instant::now();
    let parallel = (0..READS)
        .into_par_iter()
        .map(|index| get(&read_key(index)).unwrap_or_default() as u64)
        .reduce(|| 0, u64::wrapping_add);
    let parallel_time = start.elapsed();

    black_box(checksum);
    black_box(parallel);
    println!(
        "{name:<18} sequential={:>7.2} ns/get  parallel={:>7.2} ns/get",
        nanos_per_op(sequential, READS),
        nanos_per_op(parallel_time, READS),
    );
}

fn nanos_per_op(duration: Duration, operations: usize) -> f64 {
    duration.as_secs_f64() * 1_000_000_000.0 / operations as f64
}

fn populate_plain() -> HashMap<u64, SharedBlob> {
    (0..ITEMS as u64).map(|key| (key, value(key))).collect()
}

fn populate_clock(items: usize) -> ClockCache {
    let mut cache = ClockCache::new(CAPACITY_BYTES);
    for key in 0..items as u64 {
        cache.insert(key, value(key));
    }
    cache
}

fn populate_foyer(items: usize) -> FoyerCache<u64, SharedBlob> {
    let cache: FoyerCache<u64, SharedBlob> = FoyerBuilder::new(CAPACITY_BYTES)
        .with_shards(SHARDS)
        .with_eviction_config(S3FifoConfig::default())
        .with_weighter(|_, value: &SharedBlob| value.bytes.len())
        .build();
    for key in 0..items as u64 {
        cache.insert(key, value(key));
    }
    cache
}

fn populate_quick(items: usize) -> QuickCache<u64, SharedBlob, BlobWeighter> {
    let cache = QuickCache::with_weighter(ITEMS, CAPACITY_BYTES as u64, BlobWeighter);
    for key in 0..items as u64 {
        cache.insert(key, value(key));
    }
    cache
}

fn populate_moka(items: usize) -> MokaCache<u64, SharedBlob> {
    let cache = MokaCache::builder()
        .max_capacity(CAPACITY_BYTES as u64)
        .initial_capacity(ITEMS)
        .weigher(|_, value: &SharedBlob| value.bytes.len() as u32)
        .build();
    for key in 0..items as u64 {
        cache.insert(key, value(key));
    }
    cache.run_pending_tasks();
    cache
}

fn benchmark_reads() {
    println!(
        "
Read-only phase ({ITEMS} entries, {READS} lookups):"
    );

    let plain = populate_plain();
    measure_reads("HashMap borrow", |key| {
        plain.get(key).map(|value| value.bytes[0])
    });

    let clock = populate_clock(ITEMS);
    measure_reads("Custom CLOCK", |key| {
        clock.get(key).map(|value| value.bytes[0])
    });

    let foyer = populate_foyer(ITEMS);
    measure_reads("Foyer S3-FIFO", |key| {
        foyer.get(key).map(|entry| entry.value().bytes[0])
    });

    let quick = populate_quick(ITEMS);
    measure_reads("quick_cache", |key| {
        quick.get(key).map(|value| value.bytes[0])
    });

    let moka = populate_moka(ITEMS);
    measure_reads("Moka TinyLFU", |key| {
        moka.get(key).map(|value| value.bytes[0])
    });
}

fn benchmark_copy_on_write() {
    println!(
        "
Copy-on-write overlay:"
    );

    let base = populate_plain();
    for percent in [0.1, 1.0, 10.0, 100.0] {
        let updates = ((ITEMS as f64 * percent / 100.0) as usize).max(1);
        let mut overlay = HashMap::<u64, Blob>::with_capacity(updates);

        let start = Instant::now();
        for key in 0..updates as u64 {
            match overlay.entry(key) {
                Entry::Occupied(mut entry) => entry.get_mut().bytes[0] ^= 1,
                Entry::Vacant(entry) => {
                    let mut local = base[&key].as_ref().clone();
                    local.bytes[0] ^= 1;
                    entry.insert(local);
                }
            }
        }
        let first_write = start.elapsed();

        let start = Instant::now();
        for value in overlay.values_mut() {
            value.bytes[0] ^= 1;
        }
        let local_write = start.elapsed();

        println!(
            "{percent:>5.1}% ({updates:>6} values) first-write clone={first_write:?}, local update={local_write:?}"
        );
        black_box(overlay);
    }
}

fn warm_hot_set<F>(mut get: F)
where
    F: FnMut(&u64) -> bool,
{
    for _ in 0..4 {
        for key in 0..(ITEMS / 10) as u64 {
            black_box(get(&key));
        }
    }
}

fn count_hot<F>(mut get: F) -> usize
where
    F: FnMut(&u64) -> bool,
{
    (0..(ITEMS / 10) as u64).filter(|key| get(key)).count()
}

fn benchmark_eviction() {
    let hot = ITEMS / 10;
    println!(
        "
Eviction after warming {hot} keys and scanning {ITEMS} new keys (capacity={ITEMS} values):"
    );

    let mut clock = populate_clock(ITEMS);
    warm_hot_set(|key| clock.get(key).is_some());
    let start = Instant::now();
    for key in ITEMS as u64..(ITEMS * 2) as u64 {
        clock.insert(key, value(key));
    }
    let elapsed = start.elapsed();
    println!(
        "{:<18} scan insert={elapsed:?}, hot survivors={}/{}",
        "Custom CLOCK",
        count_hot(|key| clock.get(key).is_some()),
        hot
    );

    let foyer = populate_foyer(ITEMS);
    warm_hot_set(|key| foyer.get(key).is_some());
    let start = Instant::now();
    for key in ITEMS as u64..(ITEMS * 2) as u64 {
        foyer.insert(key, value(key));
    }
    let elapsed = start.elapsed();
    println!(
        "{:<18} scan insert={elapsed:?}, hot survivors={}/{}",
        "Foyer S3-FIFO",
        count_hot(|key| foyer.get(key).is_some()),
        hot
    );

    let quick = populate_quick(ITEMS);
    warm_hot_set(|key| quick.get(key).is_some());
    let start = Instant::now();
    for key in ITEMS as u64..(ITEMS * 2) as u64 {
        quick.insert(key, value(key));
    }
    let elapsed = start.elapsed();
    println!(
        "{:<18} scan insert={elapsed:?}, hot survivors={}/{}",
        "quick_cache",
        count_hot(|key| quick.get(key).is_some()),
        hot
    );

    let moka = populate_moka(ITEMS);
    warm_hot_set(|key| moka.get(key).is_some());
    moka.run_pending_tasks();
    let start = Instant::now();
    for key in ITEMS as u64..(ITEMS * 2) as u64 {
        moka.insert(key, value(key));
    }
    moka.run_pending_tasks();
    let elapsed = start.elapsed();
    println!(
        "{:<18} scan insert={elapsed:?}, hot survivors={}/{}",
        "Moka TinyLFU",
        count_hot(|key| moka.get(key).is_some()),
        hot
    );
}

fn main() {
    println!(
        "Cache comparison: {} Rayon workers, {}-byte values, {} MiB weighted capacity",
        rayon::current_num_threads(),
        VALUE_BYTES,
        CAPACITY_BYTES / 1024 / 1024
    );
    benchmark_reads();
    benchmark_copy_on_write();
    benchmark_eviction();
}
