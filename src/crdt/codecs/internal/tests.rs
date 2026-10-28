use alloy_primitives::{I256, U256};

use crate::{
    collections::delta_set::DeltaSet,
    crdt::{NonComm, Scalar, Set, state::DeltaOp},
};

use super::{
    batch::{self, InternalEncode},
    bytes, delta_op, int256, u64_set, u256,
};

fn set_delta(added: Vec<u64>, removed: Vec<u64>) -> Vec<DeltaOp<u64>> {
    added
        .into_iter()
        .map(DeltaOp::Add)
        .chain(removed.into_iter().map(DeltaOp::Sub))
        .collect()
}

#[test]
fn every_internal_type_round_trips() {
    let bytes_value = NonComm::<[u8]> {
        delta: Some(vec![1, 2, 3].into_boxed_slice()),
    };
    let encoded = bytes::encode(&bytes_value).unwrap();
    assert_eq!(
        encoded.len() as u64,
        bytes::encoded_size(&bytes_value).unwrap()
    );
    assert_eq!(bytes::decode(&encoded).unwrap(), bytes_value);

    let signed = Scalar::<I256> {
        value: I256::unchecked_from(-100),
        delta: Some(DeltaOp::Add(U256::from(25))),
        min: I256::unchecked_from(-1_000),
        max: I256::unchecked_from(1_000),
    };
    let encoded = int256::encode(&signed).unwrap();
    assert_eq!(encoded.len() as u64, int256::encoded_size(&signed).unwrap());
    assert_eq!(int256::decode(&encoded).unwrap(), signed);

    let unsigned = Scalar::<U256> {
        value: U256::from(100),
        delta: Some(DeltaOp::Sub(U256::from(25))),
        min: U256::from(10),
        max: U256::from(1_000),
    };
    let encoded = u256::encode(&unsigned).unwrap();
    assert_eq!(encoded.len() as u64, u256::encoded_size(&unsigned).unwrap());
    assert_eq!(u256::decode(&encoded).unwrap(), unsigned);

    let operations = set_delta(vec![1, 2], vec![3]);
    let encoded = delta_op::encode(&operations).unwrap();
    assert_eq!(delta_op::decode(&encoded).unwrap(), operations);

    let set = Set {
        entries: DeltaSet::try_from_slots(vec![Some(1), None, Some(3)]).unwrap(),
        delta: Some(set_delta(vec![4], vec![1])),
    };
    let encoded = u64_set::encode(&set).unwrap();
    assert_eq!(encoded.len() as u64, u64_set::encoded_size(&set).unwrap());
    assert_eq!(u64_set::decode(&encoded).unwrap(), set);
}

#[test]
fn encode_to_rejects_small_buffers() {
    let signed = Scalar::<I256>::default();
    let mut signed_output = vec![0; int256::encoded_size(&signed).unwrap() as usize - 1];
    assert_eq!(
        int256::encode_to(&signed, &mut signed_output),
        Err("output buffer too small")
    );

    let unsigned = Scalar::<U256>::default();
    let mut unsigned_output = vec![0; u256::encoded_size(&unsigned).unwrap() as usize - 1];
    assert_eq!(
        u256::encode_to(&unsigned, &mut unsigned_output),
        Err("output buffer too small")
    );
}

#[test]
fn scalar_codecs_reject_invalid_flags_truncation_and_trailing_bytes() {
    assert_eq!(int256::decode(&[16]), Err("invalid I256 flags"));
    assert_eq!(u256::decode(&[16]), Err("invalid U256 flags"));

    let signed = int256::encode(&Scalar::<I256>::default()).unwrap();
    for end in 0..signed.len() {
        assert!(int256::decode(&signed[..end]).is_err());
    }
    let mut signed_trailing = signed;
    signed_trailing.push(0);
    assert_eq!(int256::decode(&signed_trailing), Err("trailing bytes"));

    let unsigned = u256::encode(&Scalar::<U256>::default()).unwrap();
    for end in 0..unsigned.len() {
        assert!(u256::decode(&unsigned[..end]).is_err());
    }
    let mut unsigned_trailing = unsigned;
    unsigned_trailing.push(0);
    assert_eq!(u256::decode(&unsigned_trailing), Err("trailing bytes"));
}

#[test]
fn scalar_wire_layout_is_little_endian_and_fixed_width() {
    let signed = Scalar::<I256> {
        value: I256::unchecked_from(-2),
        delta: Some(DeltaOp::Sub(U256::from(3))),
        min: I256::MIN,
        max: I256::MAX,
    };
    let encoded = int256::encode(&signed).unwrap();
    assert_eq!(encoded.len(), 129);
    assert_eq!(encoded[0], 15);
    assert_eq!(
        &encoded[1..33],
        &signed.value.into_raw().to_le_bytes::<32>()
    );
    assert_eq!(&encoded[33..65], &U256::from(3).to_le_bytes::<32>());

    let unsigned = Scalar::<U256> {
        value: U256::from(2),
        delta: Some(DeltaOp::Add(U256::from(3))),
        min: U256::ZERO,
        max: U256::MAX,
    };
    let encoded = u256::encode(&unsigned).unwrap();
    assert_eq!(encoded.len(), 129);
    assert_eq!(encoded[0], 7);
    assert_eq!(&encoded[1..33], &U256::from(2).to_le_bytes::<32>());
    assert_eq!(&encoded[33..65], &U256::from(3).to_le_bytes::<32>());
}

#[test]
fn heterogeneous_batch_uses_offsets_and_round_trips_items() {
    let signed = Scalar::<I256> {
        value: I256::unchecked_from(-7),
        ..Default::default()
    };
    let unsigned = Scalar::<U256> {
        value: U256::from(9),
        ..Default::default()
    };
    let bytes_value = NonComm::<[u8]> {
        delta: Some(vec![1, 2].into_boxed_slice()),
    };
    let values: [&dyn InternalEncode; 3] = [&signed, &unsigned, &bytes_value];
    let encoded = batch::encode(&values).unwrap();
    assert_eq!(encoded.len() as u64, batch::encoded_size(&values).unwrap());

    let count = u64::from_le_bytes(encoded[0..8].try_into().unwrap()) as usize;
    assert_eq!(count, values.len());
    let header_size = (count + 1) * 8;
    let offsets = (0..count)
        .map(|index| {
            let start = 8 + index * 8;
            u64::from_le_bytes(encoded[start..start + 8].try_into().unwrap()) as usize
        })
        .collect::<Vec<_>>();
    let end = |index: usize| {
        offsets
            .get(index + 1)
            .copied()
            .unwrap_or(encoded.len() - header_size)
    };
    assert_eq!(
        int256::decode(&encoded[header_size + offsets[0]..header_size + end(0)]).unwrap(),
        signed
    );
    assert_eq!(
        u256::decode(&encoded[header_size + offsets[1]..header_size + end(1)]).unwrap(),
        unsigned
    );
    assert_eq!(
        bytes::decode(&encoded[header_size + offsets[2]..header_size + end(2)]).unwrap(),
        bytes_value
    );
}
