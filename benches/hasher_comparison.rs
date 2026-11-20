use std::{
    collections::HashMap,
    hash::{BuildHasher, BuildHasherDefault, Hash, Hasher},
    hint::black_box,
    time::{Duration, Instant},
};

use xxhash_rust::xxh3::{Xxh3Default, Xxh3DefaultBuilder};

const ITEMS: usize = 4_096;
const HASH_OPS: usize = 1_000_000;
const LOOKUP_OPS: usize = 500_000;
const SAMPLES: usize = 5;

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

fn make_keys(segment_len: usize) -> Vec<Vec<String>> {
    (0..ITEMS)
        .map(|index| {
            (0..4)
                .map(|part| make_segment(index as u64, part as u64, segment_len))
                .collect()
        })
        .collect()
}

fn make_segment(index: u64, part: u64, len: usize) -> String {
    let mut value = index
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(part.wrapping_mul(0xbf58_476d_1ce4_e5b9));
    let mut bytes = Vec::with_capacity(len);

    for _ in 0..len {
        value ^= value >> 12;
        value ^= value << 25;
        value ^= value >> 27;
        value = value.wrapping_mul(0x2545_f491_4f6c_dd1d);
        bytes.push(b'a' + (value % 26) as u8);
    }

    String::from_utf8(bytes).unwrap()
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn nanos_per_op(duration: Duration, operations: usize) -> f64 {
    duration.as_secs_f64() * 1_000_000_000.0 / operations as f64
}

fn time_hashes<H: Default + Hasher>(keys: &[Vec<String>]) -> f64 {
    let rounds = HASH_OPS.div_ceil(keys.len());
    let operations = rounds * keys.len();
    let mut samples = Vec::with_capacity(SAMPLES);

    for _ in 0..SAMPLES {
        let start = Instant::now();
        let mut checksum = 0u64;

        for _ in 0..rounds {
            for key in keys {
                let mut hasher = H::default();
                key.hash(&mut hasher);
                checksum = checksum.wrapping_add(hasher.finish());
            }
        }

        black_box(checksum);
        samples.push(nanos_per_op(start.elapsed(), operations));
    }

    median(samples)
}

fn build_map<S: BuildHasher + Default>(
    keys: &[Vec<String>],
) -> (HashMap<Vec<String>, usize, S>, f64) {
    let mut map = HashMap::with_capacity_and_hasher(keys.len(), S::default());
    let start = Instant::now();

    for (value, key) in keys.iter().cloned().enumerate() {
        map.insert(key, value);
    }

    let elapsed = nanos_per_op(start.elapsed(), keys.len());
    (map, elapsed)
}

fn time_lookups<S: BuildHasher>(map: &HashMap<Vec<String>, usize, S>, keys: &[Vec<String>]) -> f64 {
    let rounds = LOOKUP_OPS.div_ceil(keys.len());
    let operations = rounds * keys.len();
    let mut samples = Vec::with_capacity(SAMPLES);

    for _ in 0..SAMPLES {
        let start = Instant::now();
        let mut checksum = 0usize;

        for _ in 0..rounds {
            for key in keys {
                checksum = checksum.wrapping_add(*map.get(key).unwrap());
            }
        }

        black_box(checksum);
        samples.push(nanos_per_op(start.elapsed(), operations));
    }

    median(samples)
}

fn collision_stats<H: Default + Hasher>(keys: &[Vec<String>]) -> (usize, usize) {
    let mut counts = HashMap::<u64, usize>::new();

    for key in keys {
        let mut hasher = H::default();
        key.hash(&mut hasher);
        *counts.entry(hasher.finish()).or_default() += 1;
    }

    let largest_group = counts.values().copied().max().unwrap_or_default();
    (counts.len(), largest_group)
}

fn benchmark_case(name: &str, keys: &[Vec<String>]) {
    let sum_hash = time_hashes::<SumHasher>(keys);
    let xxh3_hash = time_hashes::<Xxh3Default>(keys);

    let (sum_map, sum_insert) = build_map::<SumBuildHasher>(keys);
    let (xxh3_map, xxh3_insert) = build_map::<Xxh3DefaultBuilder>(keys);
    let sum_lookup = time_lookups(&sum_map, keys);
    let xxh3_lookup = time_lookups(&xxh3_map, keys);

    let (sum_unique, sum_largest) = collision_stats::<SumHasher>(keys);
    let (xxh3_unique, xxh3_largest) = collision_stats::<Xxh3Default>(keys);

    println!("\n{name} ({ITEMS} keys, 4 strings/key):");
    println!(
        "{:<10} hash={:>8.2} ns  insert={:>10.2} ns  lookup={:>10.2} ns  unique={:>4}  largest-collision-group={}",
        "SumHasher", sum_hash, sum_insert, sum_lookup, sum_unique, sum_largest
    );
    println!(
        "{:<10} hash={:>8.2} ns  insert={:>10.2} ns  lookup={:>10.2} ns  unique={:>4}  largest-collision-group={}",
        "XXH3", xxh3_hash, xxh3_insert, xxh3_lookup, xxh3_unique, xxh3_largest
    );
    println!(
        "XXH3/Sum raw-hash ratio: {:.2}x; Sum/XXH3 lookup ratio: {:.2}x",
        xxh3_hash / sum_hash,
        sum_lookup / xxh3_lookup
    );
}

fn main() {
    println!("SumHasher vs XXH3 (median of {SAMPLES} timed samples; release profile)");

    let short_keys = make_keys(12);
    benchmark_case("Short strings (12 bytes each)", &short_keys);

    let long_keys = make_keys(128);
    benchmark_case("Long strings (128 bytes each)", &long_keys);
}
