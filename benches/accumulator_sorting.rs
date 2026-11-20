use alloy_primitives::U256;
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
    hint::black_box,
    time::{Duration, Instant},
};

const KEYS: usize = 100_000;
const ENTRIES_PER_KEY: usize = 10;
const ENTRIES: usize = KEYS * ENTRIES_PER_KEY;
const SAMPLES: usize = 5;

#[derive(Clone, Copy, Eq, PartialEq)]
struct SortKey {
    delta_abs_u64: u64,
    delta_abs_u256: U256,
    tx_id: u64,
    entry_id: usize,
}

impl Ord for SortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.delta_abs_u64
            .cmp(&other.delta_abs_u64)
            .then_with(|| {
                if self.delta_abs_u64 == u64::MAX {
                    self.delta_abs_u256.cmp(&other.delta_abs_u256)
                } else {
                    Ordering::Equal
                }
            })
            .then_with(|| self.tx_id.cmp(&other.tx_id))
            .then_with(|| self.entry_id.cmp(&other.entry_id))
    }
}

impl PartialOrd for SortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy)]
struct Timing {
    insert: Duration,
    sort: Duration,
}

impl Timing {
    fn total(self) -> Duration {
        self.insert + self.sort
    }
}

fn mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn input() -> Vec<(u64, SortKey)> {
    (0..ENTRIES)
        .map(|index| {
            let entry_id = index * 999_983 % ENTRIES;
            let key = (entry_id % KEYS) as u64;
            let random = mix(entry_id as u64);
            let delta_abs_u256 = if entry_id % 16 == 0 {
                U256::from(u64::MAX) + U256::from(random)
            } else {
                U256::from(random)
            };
            let delta_abs_u64 = if delta_abs_u256 > U256::from(u64::MAX) {
                u64::MAX
            } else {
                delta_abs_u256.as_limbs()[0]
            };

            (
                key,
                SortKey {
                    delta_abs_u64,
                    delta_abs_u256,
                    tx_id: mix(random) % 1_000_000,
                    entry_id,
                },
            )
        })
        .collect()
}

fn vec_then_sort(input: &[(u64, SortKey)]) -> (Timing, u64) {
    let mut by_key = HashMap::<u64, Vec<SortKey>>::with_capacity(KEYS);

    let start = Instant::now();
    for &(key, sort_key) in input {
        by_key
            .entry(key)
            .or_insert_with(|| Vec::with_capacity(ENTRIES_PER_KEY))
            .push(sort_key);
    }
    let insert = start.elapsed();

    let start = Instant::now();
    for entries in by_key.values_mut() {
        entries.sort_unstable();
    }
    let sort = start.elapsed();

    let checksum = checksum_vec(&by_key);
    black_box(by_key);
    (Timing { insert, sort }, checksum)
}

fn btree_on_insert(input: &[(u64, SortKey)]) -> (Timing, u64) {
    let mut by_key = HashMap::<u64, BTreeSet<SortKey>>::with_capacity(KEYS);

    let start = Instant::now();
    for &(key, sort_key) in input {
        by_key.entry(key).or_default().insert(sort_key);
    }
    let insert = start.elapsed();

    let checksum = checksum_btree(&by_key);
    black_box(by_key);
    (
        Timing {
            insert,
            sort: Duration::ZERO,
        },
        checksum,
    )
}

fn checksum_vec(by_key: &HashMap<u64, Vec<SortKey>>) -> u64 {
    (0..KEYS as u64).fold(0, |checksum, key| {
        by_key[&key].iter().fold(checksum, |checksum, entry| {
            checksum.rotate_left(7) ^ entry.entry_id as u64
        })
    })
}

fn checksum_btree(by_key: &HashMap<u64, BTreeSet<SortKey>>) -> u64 {
    (0..KEYS as u64).fold(0, |checksum, key| {
        by_key[&key].iter().fold(checksum, |checksum, entry| {
            checksum.rotate_left(7) ^ entry.entry_id as u64
        })
    })
}

fn median_duration(mut samples: Vec<Duration>) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn nanos_per_entry(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000_000_000.0 / ENTRIES as f64
}

fn print_row(name: &str, insert: Duration, sort: Duration) {
    let total = insert + sort;

    println!(
        "{name:<34} {:>10.3?} {:>10.3?} {:>10.3?} {:>12.2}",
        insert,
        sort,
        total,
        nanos_per_entry(total),
    );
}

fn main() {
    let input = input();
    let mut vec_insert = Vec::with_capacity(SAMPLES);
    let mut vec_sort = Vec::with_capacity(SAMPLES);
    let mut btree_insert = Vec::with_capacity(SAMPLES);

    let (_, expected) = vec_then_sort(&input);
    let (_, actual) = btree_on_insert(&input);
    assert_eq!(actual, expected);

    for sample in 0..SAMPLES {
        if sample % 2 == 0 {
            let (vec_timing, vec_checksum) = vec_then_sort(&input);
            let (btree_timing, btree_checksum) = btree_on_insert(&input);
            assert_eq!(btree_checksum, vec_checksum);
            vec_insert.push(vec_timing.insert);
            vec_sort.push(vec_timing.sort);
            btree_insert.push(btree_timing.insert);
        } else {
            let (btree_timing, btree_checksum) = btree_on_insert(&input);
            let (vec_timing, vec_checksum) = vec_then_sort(&input);
            assert_eq!(btree_checksum, vec_checksum);
            btree_insert.push(btree_timing.insert);
            vec_insert.push(vec_timing.insert);
            vec_sort.push(vec_timing.sort);
        }
    }

    let vec_timing = Timing {
        insert: median_duration(vec_insert),
        sort: median_duration(vec_sort),
    };
    let btree_timing = Timing {
        insert: median_duration(btree_insert),
        sort: Duration::ZERO,
    };

    println!(
        "Accumulator ordering benchmark: {ENTRIES} entries, {KEYS} keys, \
         {ENTRIES_PER_KEY} entries/key, median of {SAMPLES} samples\n"
    );
    println!(
        "{:<34} {:>10} {:>10} {:>10} {:>12}",
        "Scenario", "insert", "sort", "total", "ns/entry"
    );
    print_row(
        "HashMap<K, Vec> + final sort",
        vec_timing.insert,
        vec_timing.sort,
    );
    print_row(
        "HashMap<K, BTreeSet> insertion",
        btree_timing.insert,
        btree_timing.sort,
    );
    println!(
        "\nBTree/Vec total-time ratio: {:.2}x",
        btree_timing.total().as_secs_f64() / vec_timing.total().as_secs_f64()
    );
}
