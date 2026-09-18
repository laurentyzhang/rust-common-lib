use std::{hint::black_box, time::Instant};

use alloy_primitives::U256 as AlloyU256;
use alloy_rlp::{Decodable, Encodable, decode_exact, encode};
use rust_common_lib::crdt::{
    bytes::Bytes, codecs::internal, crdt::Crdt, int64::I64, state::DeltaOp, u64_set::U64Set,
    u256::U256, uint64::U64,
};

const ITERATIONS: usize = 10_000;

fn compare<T>(
    name: &str,
    value: &T,
    internal_size: fn(&T) -> internal::Result<u64>,
    internal_encode: fn(&T) -> internal::Result<Vec<u8>>,
    internal_encode_to: fn(&T, &mut [u8]) -> internal::Result<u64>,
    internal_decode: fn(&[u8]) -> internal::Result<T>,
) where
    T: Encodable + Decodable + PartialEq,
{
    let rlp = encode(value);
    let internal = internal_encode(value).unwrap();
    assert!(internal_decode(&internal).unwrap().eq(value));
    decode_exact::<T>(&rlp).unwrap();

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        black_box(encode(black_box(value)));
    }
    let rlp_allocating_encode = start.elapsed();

    let mut rlp_output = Vec::with_capacity(value.length());
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        rlp_output.clear();
        black_box(value).encode(&mut rlp_output);
        black_box(&rlp_output);
    }
    let rlp_reused_encode = start.elapsed();

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        black_box(internal_encode(black_box(value)).unwrap());
    }
    let internal_allocating_encode = start.elapsed();

    let mut internal_output = vec![
        0;
        usize::try_from(internal_size(value).unwrap())
            .expect("encoded size exceeds usize::MAX")
    ];
    let start = Instant::now();
    for _ in 0..ITERATIONS {
        black_box(internal_encode_to(black_box(value), black_box(&mut internal_output)).unwrap());
    }
    let internal_reused_encode = start.elapsed();

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        black_box(decode_exact::<T>(black_box(&rlp)).unwrap());
    }
    let rlp_decode = start.elapsed();

    let start = Instant::now();
    for _ in 0..ITERATIONS {
        black_box(internal_decode(black_box(&internal)).unwrap());
    }
    let internal_decode_time = start.elapsed();

    println!(
        "{name}: size RLP={} B, internal={} B | allocating encode RLP={rlp_allocating_encode:?}, internal={internal_allocating_encode:?} | reused encode RLP={rlp_reused_encode:?}, internal={internal_reused_encode:?} | decode RLP={rlp_decode:?}, internal={internal_decode_time:?}",
        rlp.len(),
        internal.len()
    );
}

#[test]
fn compare_internal_codec_with_rlp_on_same_objects() {
    let mut bytes = Bytes::default();
    bytes.add_delta(&[1; 128]).unwrap();
    bytes.apply_delta();
    bytes.add_delta(&[2; 64]).unwrap();
    compare(
        "Bytes",
        &bytes,
        internal::bytes::encoded_size,
        internal::bytes::encode,
        internal::bytes::encode_to,
        internal::bytes::decode,
    );

    let mut int64 = I64::default();
    int64.add_delta(&1_000).unwrap();
    int64.apply_delta();
    int64.add_delta(&-250).unwrap();
    compare(
        "I64",
        &int64,
        internal::int64::encoded_size,
        internal::int64::encode,
        internal::int64::encode_to,
        internal::int64::decode,
    );

    let mut uint64 = U64::default();
    uint64.add_delta(&1_000).unwrap();
    uint64.apply_delta();
    uint64.add_delta(&250).unwrap();
    compare(
        "U64",
        &uint64,
        internal::uint64::encoded_size,
        internal::uint64::encode,
        internal::uint64::encode_to,
        internal::uint64::decode,
    );

    let mut u256 = U256::default();
    u256.add_delta(&AlloyU256::from(1_000)).unwrap();
    u256.apply_delta();
    u256.add_delta(&AlloyU256::from(250)).unwrap();
    compare(
        "U256",
        &u256,
        internal::u256::encoded_size,
        internal::u256::encode,
        internal::u256::encode_to,
        internal::u256::decode,
    );

    let mut path = U64Set::new().unwrap();
    let initial_delta = (0..64).map(DeltaOp::Add).collect::<Vec<_>>();
    path.add_delta(&initial_delta).unwrap();
    path.apply_delta();
    let pending_delta = (100..132)
        .map(DeltaOp::Add)
        .chain((0..16).map(DeltaOp::Sub))
        .collect::<Vec<_>>();
    path.add_delta(&pending_delta).unwrap();
    compare(
        "U64Set",
        &path,
        internal::path_meta::encoded_size,
        internal::path_meta::encode,
        internal::path_meta::encode_to,
        internal::path_meta::decode,
    );
}
