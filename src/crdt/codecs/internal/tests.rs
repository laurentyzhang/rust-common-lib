use alloy_primitives::U256 as AlloyU256;

use crate::{
    collections::delta_set::DeltaSet,
    crdt::{bytes::Bytes, int64::I64, state::DeltaOp, u64_set::U64Set, u256::U256, uint64::U64},
};

fn set_delta(added: Vec<u64>, removed: Vec<u64>) -> Vec<DeltaOp<u64>> {
    added
        .into_iter()
        .map(DeltaOp::Add)
        .chain(removed.into_iter().map(DeltaOp::Sub))
        .collect()
}

use super::{
    batch::{self, InternalEncode},
    bytes, int64, path_delta, path_meta, u256, uint64,
};

#[test]
fn every_internal_type_round_trips() {
    let bytes_value = Bytes {
        delta: Some(vec![1, 2, 3].into_boxed_slice()),
    };
    let encoded = bytes::encode(&bytes_value).unwrap();
    assert_eq!(
        encoded.len() as u64,
        bytes::encoded_size(&bytes_value).unwrap()
    );
    assert!(bytes::decode(&encoded).unwrap() == bytes_value);

    let int64_value = I64 {
        value: -100,
        delta: 25,
        limits: (-1_000, 1_000),
    };
    let encoded = int64::encode(&int64_value).unwrap();
    assert_eq!(
        encoded.len() as u64,
        int64::encoded_size(&int64_value).unwrap()
    );
    assert!(int64::decode(&encoded).unwrap() == int64_value);

    let uint64_value = U64 {
        value: 100,
        delta: 25,
        delta_subtract: false,
        limits: (0, 1_000),
    };
    let encoded = uint64::encode(&uint64_value).unwrap();
    assert_eq!(
        encoded.len() as u64,
        uint64::encoded_size(&uint64_value).unwrap()
    );
    assert!(uint64::decode(&encoded).unwrap() == uint64_value);

    let u256_value = U256 {
        value: AlloyU256::from(1_u64) << 200,
        delta: AlloyU256::from(25),
        delta_subtract: false,
        limits: (AlloyU256::ZERO, AlloyU256::MAX),
    };
    let encoded = u256::encode(&u256_value).unwrap();
    assert_eq!(
        encoded.len() as u64,
        u256::encoded_size(&u256_value).unwrap()
    );
    assert!(u256::decode(&encoded).unwrap() == u256_value);

    let delta = set_delta(vec![40, 50], vec![10]);
    let encoded = path_delta::encode(&delta).unwrap();
    assert_eq!(
        encoded.len() as u64,
        path_delta::encoded_size(&delta).unwrap()
    );
    assert!(path_delta::decode(&encoded).unwrap() == delta);

    let mut entries = DeltaSet::try_from_elements(vec![10, 20, 30]).unwrap();
    entries.remove(&20);
    entries.commit();
    let path = U64Set {
        entries,
        delta: Some(delta),
    };
    let encoded = path_meta::encode(&path).unwrap();
    assert_eq!(
        encoded.len() as u64,
        path_meta::encoded_size(&path).unwrap()
    );
    let decoded = path_meta::decode(&encoded).unwrap();
    assert!(decoded == path);
    assert_eq!(decoded.entries.get_at(1), None);
    assert_eq!(decoded.entries.get_at(2), Some(&30));
}

#[test]
fn encode_to_rejects_short_buffers_without_writing() {
    let bytes_value = Bytes {
        delta: Some(vec![1, 2, 3].into_boxed_slice()),
    };
    let mut output =
        vec![0xaa; usize::try_from(bytes::encoded_size(&bytes_value).unwrap()).unwrap() - 1];
    assert_eq!(
        bytes::encode_to(&bytes_value, &mut output),
        Err("output buffer too small")
    );
    assert!(output.iter().all(|byte| *byte == 0xaa));

    let int64_value = I64 {
        value: 10,
        delta: 2,
        limits: (0, 100),
    };
    let mut output =
        vec![0xaa; usize::try_from(int64::encoded_size(&int64_value).unwrap()).unwrap() - 1];
    assert_eq!(
        int64::encode_to(&int64_value, &mut output),
        Err("output buffer too small")
    );
    assert!(output.iter().all(|byte| *byte == 0xaa));

    let uint64_value = U64 {
        value: 10,
        delta: 2,
        delta_subtract: false,
        limits: (0, 100),
    };
    let mut output =
        vec![0xaa; usize::try_from(uint64::encoded_size(&uint64_value).unwrap()).unwrap() - 1];
    assert_eq!(
        uint64::encode_to(&uint64_value, &mut output),
        Err("output buffer too small")
    );
    assert!(output.iter().all(|byte| *byte == 0xaa));

    let u256_value = U256 {
        value: AlloyU256::from(10),
        delta: AlloyU256::from(2),
        delta_subtract: false,
        limits: (AlloyU256::ZERO, AlloyU256::from(100)),
    };
    let mut output =
        vec![0xaa; usize::try_from(u256::encoded_size(&u256_value).unwrap()).unwrap() - 1];
    assert_eq!(
        u256::encode_to(&u256_value, &mut output),
        Err("output buffer too small")
    );
    assert!(output.iter().all(|byte| *byte == 0xaa));

    let delta = set_delta(vec![1], vec![2]);
    let mut output =
        vec![0xaa; usize::try_from(path_delta::encoded_size(&delta).unwrap()).unwrap() - 1];
    assert_eq!(
        path_delta::encode_to(&delta, &mut output),
        Err("output buffer too small")
    );
    assert!(output.iter().all(|byte| *byte == 0xaa));

    let path = U64Set {
        entries: DeltaSet::try_from_slots(vec![Some(1), None, Some(2)]).unwrap(),
        delta: Some(delta),
    };
    let mut output =
        vec![0xaa; usize::try_from(path_meta::encoded_size(&path).unwrap()).unwrap() - 1];
    assert_eq!(
        path_meta::encode_to(&path, &mut output),
        Err("output buffer too small")
    );
    assert!(output.iter().all(|byte| *byte == 0xaa));

    let values: [&dyn InternalEncode; 2] = [&bytes_value, &uint64_value];
    let mut output =
        vec![0xaa; usize::try_from(batch::encoded_size(&values).unwrap()).unwrap() - 1];
    assert_eq!(
        batch::encode_to(&values, &mut output),
        Err("output buffer too small")
    );
    assert!(output.iter().all(|byte| *byte == 0xaa));
}

#[test]
fn malformed_input_is_rejected() {
    assert!(matches!(bytes::decode(&[2]), Err("invalid Bytes flags")));
    assert!(matches!(int64::decode(&[8]), Err("invalid I64 flags")));
    assert!(matches!(uint64::decode(&[8]), Err("invalid U64 flags")));
    assert!(matches!(u256::decode(&[8]), Err("invalid U256 flags")));

    let mut invalid_path = vec![0];
    invalid_path.extend_from_slice(&3_u64.to_le_bytes());
    invalid_path.push(0b1111_1101);
    assert!(matches!(
        path_meta::decode(&invalid_path),
        Err("invalid U64Set bitmap")
    ));

    let mut duplicate_path = vec![0];
    duplicate_path.extend_from_slice(&2_u64.to_le_bytes());
    duplicate_path.push(0b0000_0011);
    duplicate_path.extend_from_slice(&7_u64.to_le_bytes());
    duplicate_path.extend_from_slice(&7_u64.to_le_bytes());
    assert!(matches!(
        path_meta::decode(&duplicate_path),
        Err("duplicate or unindexable U64Set entry")
    ));

    assert!(path_delta::decode(&u64::MAX.to_le_bytes()).is_err());

    let valid = I64 {
        value: 1,
        delta: 0,
        limits: (i64::MIN, i64::MAX),
    };
    let mut trailing = int64::encode(&valid).unwrap();
    trailing.push(0);
    assert!(matches!(int64::decode(&trailing), Err("trailing bytes")));
}

#[test]
fn every_truncated_encoding_is_rejected() {
    let bytes_encoded = bytes::encode(&Bytes {
        delta: Some(vec![1, 2, 3].into_boxed_slice()),
    })
    .unwrap();
    for end in 0..bytes_encoded.len() {
        assert!(bytes::decode(&bytes_encoded[..end]).is_err());
    }

    let int64_encoded = int64::encode(&I64 {
        value: -10,
        delta: 2,
        limits: (-100, 100),
    })
    .unwrap();
    for end in 0..int64_encoded.len() {
        assert!(int64::decode(&int64_encoded[..end]).is_err());
    }

    let uint64_encoded = uint64::encode(&U64 {
        value: 10,
        delta: 2,
        delta_subtract: false,
        limits: (0, 100),
    })
    .unwrap();
    for end in 0..uint64_encoded.len() {
        assert!(uint64::decode(&uint64_encoded[..end]).is_err());
    }

    let u256_encoded = u256::encode(&U256 {
        value: AlloyU256::from(10),
        delta: AlloyU256::from(2),
        delta_subtract: false,
        limits: (AlloyU256::ZERO, AlloyU256::from(100)),
    })
    .unwrap();
    for end in 0..u256_encoded.len() {
        assert!(u256::decode(&u256_encoded[..end]).is_err());
    }

    let delta_encoded = path_delta::encode(&set_delta(vec![1, 2], vec![3])).unwrap();
    for end in 0..delta_encoded.len() {
        assert!(path_delta::decode(&delta_encoded[..end]).is_err());
    }

    let path_encoded = path_meta::encode(&U64Set {
        entries: DeltaSet::try_from_slots(vec![Some(1), None, Some(2)]).unwrap(),
        delta: Some(set_delta(vec![3], vec![1])),
    })
    .unwrap();
    for end in 0..path_encoded.len() {
        assert!(path_meta::decode(&path_encoded[..end]).is_err());
    }
}

#[test]
fn primitive_encodings_match_golden_bytes() {
    let bytes_value = Bytes {
        delta: Some(vec![0xaa, 0xbb].into_boxed_slice()),
    };
    let mut expected = vec![1];
    expected.extend_from_slice(&2_u64.to_le_bytes());
    expected.extend_from_slice(&[0xaa, 0xbb]);
    assert_eq!(bytes::encode(&bytes_value).unwrap(), expected);

    let int64_value = I64 {
        value: -2,
        delta: 0,
        limits: (i64::MIN, i64::MAX),
    };
    let mut expected = vec![7];
    expected.extend_from_slice(&(-2_i64).to_le_bytes());
    expected.extend_from_slice(&0_i64.to_le_bytes());
    expected.extend_from_slice(&i64::MIN.to_le_bytes());
    expected.extend_from_slice(&i64::MAX.to_le_bytes());
    assert_eq!(int64::encode(&int64_value).unwrap(), expected);

    let uint64_value = U64 {
        value: 1,
        delta: 2,
        delta_subtract: false,
        limits: (3, 4),
    };
    let mut expected = vec![7];
    expected.extend_from_slice(&1_u64.to_le_bytes());
    expected.extend_from_slice(&2_u64.to_le_bytes());
    expected.extend_from_slice(&3_u64.to_le_bytes());
    expected.extend_from_slice(&4_u64.to_le_bytes());
    assert_eq!(uint64::encode(&uint64_value).unwrap(), expected);

    let u256_value = U256 {
        value: AlloyU256::from(1),
        delta: AlloyU256::ZERO,
        delta_subtract: false,
        limits: (AlloyU256::ZERO, AlloyU256::MAX),
    };
    let mut expected = vec![7];
    expected.extend_from_slice(&AlloyU256::from(1).to_le_bytes::<32>());
    expected.extend_from_slice(&AlloyU256::ZERO.to_le_bytes::<32>());
    expected.extend_from_slice(&AlloyU256::ZERO.to_le_bytes::<32>());
    expected.extend_from_slice(&AlloyU256::MAX.to_le_bytes::<32>());
    assert_eq!(u256::encode(&u256_value).unwrap(), expected);
}

#[test]
fn path_and_batch_encodings_match_golden_bytes() {
    let delta = set_delta(vec![1], vec![2]);
    let mut expected_delta = Vec::new();
    expected_delta.extend_from_slice(&1_u64.to_le_bytes());
    expected_delta.extend_from_slice(&1_u64.to_le_bytes());
    expected_delta.extend_from_slice(&1_u64.to_le_bytes());
    expected_delta.extend_from_slice(&2_u64.to_le_bytes());
    assert_eq!(path_delta::encode(&delta).unwrap(), expected_delta);

    let path = U64Set {
        entries: DeltaSet::try_from_slots(vec![Some(10), None, Some(30)]).unwrap(),
        delta: None,
    };
    let mut expected_path = vec![0];
    expected_path.extend_from_slice(&3_u64.to_le_bytes());
    expected_path.push(0b0000_0101);
    expected_path.extend_from_slice(&10_u64.to_le_bytes());
    expected_path.extend_from_slice(&30_u64.to_le_bytes());
    assert_eq!(path_meta::encode(&path).unwrap(), expected_path);

    let bytes_value = Bytes {
        delta: Some(vec![0xaa].into_boxed_slice()),
    };
    let uint64_value = U64 {
        value: 7,
        delta: 0,
        delta_subtract: false,
        limits: (u64::MIN, u64::MAX),
    };
    let values: [&dyn InternalEncode; 2] = [&bytes_value, &uint64_value];

    let mut expected_batch = Vec::new();
    expected_batch.extend_from_slice(&2_u64.to_le_bytes());
    expected_batch.extend_from_slice(&0_u64.to_le_bytes());
    expected_batch.extend_from_slice(&10_u64.to_le_bytes());
    expected_batch.push(1);
    expected_batch.extend_from_slice(&1_u64.to_le_bytes());
    expected_batch.push(0xaa);
    expected_batch.push(7);
    expected_batch.extend_from_slice(&7_u64.to_le_bytes());
    expected_batch.extend_from_slice(&0_u64.to_le_bytes());
    expected_batch.extend_from_slice(&u64::MIN.to_le_bytes());
    expected_batch.extend_from_slice(&u64::MAX.to_le_bytes());
    assert_eq!(batch::encode(&values).unwrap(), expected_batch);
}

#[test]
fn large_batch_uses_non_overlapping_parallel_sections() {
    let stored = vec![
        U64 {
            value: 7,
            delta: 0,
            delta_subtract: false,
            limits: (u64::MIN, u64::MAX),
        };
        2_048
    ];
    let values = stored
        .iter()
        .map(|value| value as &dyn InternalEncode)
        .collect::<Vec<_>>();
    let encoded = batch::encode(&values).unwrap();

    let header_size = (values.len() + 1) * 8;
    assert_eq!(
        u64::from_le_bytes(encoded[..8].try_into().unwrap()),
        values.len() as u64
    );
    assert_eq!(u64::from_le_bytes(encoded[8..16].try_into().unwrap()), 0);
    let last_offset_start = values.len() * 8;
    assert_eq!(
        u64::from_le_bytes(
            encoded[last_offset_start..last_offset_start + 8]
                .try_into()
                .unwrap()
        ),
        ((values.len() - 1) * 33) as u64
    );
    assert!(uint64::decode(&encoded[header_size..header_size + 33]).unwrap() == stored[0]);
    assert!(uint64::decode(&encoded[encoded.len() - 33..]).unwrap() == stored[stored.len() - 1]);
}
