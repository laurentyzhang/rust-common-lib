use std::borrow::Cow;

use crate::crdt::state::{
    Numeric, Status, Tracked, Value, op::Operations, status::Tag, value::Values,
};

use super::{Reader, Result, Writer, bytes, int64, u64_set, u256, uint64};

const BYTES: u8 = 1;
const I64: u8 = 2;
const U64: u8 = 3;
const U256: u8 = 4;
const U64_SET: u8 = 5;
const DELETED: u8 = 6;
const DEFAULT: u8 = 7;
const STRIPPED: u8 = 8;
const MISSING: u8 = 9;

const HEADER_SIZE: u64 = 34;

fn value_tag(value: &Status) -> u8 {
    match value {
        Status::Value(Value::Bytes(_)) => BYTES,
        Status::Value(Value::Numeric(Numeric::I64(_))) => I64,
        Status::Value(Value::Numeric(Numeric::U64(_))) => U64,
        Status::Value(Value::Numeric(Numeric::U256(_))) => U256,
        Status::Value(Value::U64Set(_)) => U64_SET,
        Status::Value(Value::None) | Status::Tag(Tag::Missing) => MISSING,
        Status::Tag(Tag::Deleted) => DELETED,
        Status::Tag(Tag::Default) => DEFAULT,
        Status::Tag(Tag::Stripped) => STRIPPED,
    }
}

fn value_size(value: &Status) -> Result<u64> {
    match value {
        Status::Tag(_) | Status::Value(Value::None) => Ok(0),
        Status::Value(Value::Bytes(value)) => bytes::encoded_size(value),
        Status::Value(Value::Numeric(Numeric::I64(value))) => int64::encoded_size(value),
        Status::Value(Value::Numeric(Numeric::U64(value))) => uint64::encoded_size(value),
        Status::Value(Value::Numeric(Numeric::U256(value))) => u256::encoded_size(value),
        Status::Value(Value::U64Set(value)) => u64_set::encoded_size(value),
    }
}

fn encode_value_to(value: &Status, output: &mut [u8]) -> Result<u64> {
    match value {
        Status::Tag(_) | Status::Value(Value::None) => Ok(0),
        Status::Value(Value::Bytes(value)) => bytes::encode_to(value, output),
        Status::Value(Value::Numeric(Numeric::I64(value))) => int64::encode_to(value, output),
        Status::Value(Value::Numeric(Numeric::U64(value))) => uint64::encode_to(value, output),
        Status::Value(Value::Numeric(Numeric::U256(value))) => u256::encode_to(value, output),
        Status::Value(Value::U64Set(value)) => u64_set::encode_to(value, output),
    }
}

pub fn encoded_size(tracked: &Tracked<Status, Status>) -> Result<u64> {
    let original_size = value_size(&tracked.value.original)?;
    let current_size = value_size(&tracked.value.current)?;
    HEADER_SIZE
        .checked_add(original_size)
        .and_then(|size| size.checked_add(current_size))
        .ok_or("encoded size overflow")
}

pub fn encode(tracked: &Tracked<Status, Status>) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(tracked)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(tracked, &mut output)?;
    Ok(output)
}

pub fn encode_to(tracked: &Tracked<Status, Status>, output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(tracked)?;
    let size_usize = usize::try_from(size).map_err(|_| "encoded size exceeds usize::MAX")?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let mut writer = Writer::new(output);
    let original_size = value_size(&tracked.value.original)?;
    writer.write_u8(value_tag(&tracked.value.original))?;
    writer.write_u8(value_tag(&tracked.value.current))?;
    writer.write_u64(original_size)?;
    writer.write_u64(tracked.id)?;
    writer.write_u32(tracked.operations.reads.count())?;
    writer.write_u32(tracked.operations.existence_checks.count())?;
    writer.write_u32(tracked.operations.removes.count())?;
    writer.write_u32(tracked.operations.deltas.count())?;
    let header_size = writer.finish();

    let payload = &mut output[header_size..size_usize];
    let original_size_usize =
        usize::try_from(original_size).map_err(|_| "encoded size exceeds usize::MAX")?;
    let (original_payload, current_payload) = payload.split_at_mut(original_size_usize);
    let original_written = encode_value_to(&tracked.value.original, original_payload)?;
    let current_written = encode_value_to(&tracked.value.current, current_payload)?;
    if HEADER_SIZE + original_written + current_written != size {
        return Err("encoded size mismatch");
    }
    Ok(size)
}

pub fn decode(input: &[u8]) -> Result<Tracked<Status, Status>> {
    let mut reader = Reader::new(input);
    let original_tag = reader.read_u8()?;
    let current_tag = reader.read_u8()?;
    let original_size = reader.read_u64()?;
    let id = reader.read_u64()?;
    let reads = reader.read_u32()?;
    let existence_checks = reader.read_u32()?;
    let deletes = reader.read_u32()?;
    let deltas = reader.read_u32()?;
    let payload = reader.read_bytes(reader.remaining())?;
    reader.finish()?;

    let original_size =
        usize::try_from(original_size).map_err(|_| "encoded size exceeds usize::MAX")?;
    if original_size > payload.len() {
        return Err("invalid original value size");
    }
    let (original_payload, current_payload) = payload.split_at(original_size);
    let original = decode_value(original_tag, original_payload)?;
    let current = decode_value(current_tag, current_payload)?;

    let tracked = Tracked {
        id,
        value: Values { original, current },
        operations: Operations {
            reads: reads.into(),
            existence_checks: existence_checks.into(),
            removes: deletes.into(),
            deltas: deltas.into(),
        },
    };
    Ok(tracked)
}

fn decode_value(tag: u8, payload: &[u8]) -> Result<Status> {
    let value = match tag {
        BYTES => Status::Value(Value::Bytes(Cow::Owned(bytes::decode(payload)?))),
        I64 => Status::Value(Value::Numeric(Numeric::I64(Cow::Owned(int64::decode(
            payload,
        )?)))),
        U64 => Status::Value(Value::Numeric(Numeric::U64(Cow::Owned(uint64::decode(
            payload,
        )?)))),
        U256 => Status::Value(Value::Numeric(Numeric::U256(Cow::Owned(u256::decode(
            payload,
        )?)))),
        U64_SET => Status::Value(Value::U64Set(Cow::Owned(u64_set::decode(payload)?))),
        DELETED if payload.is_empty() => Status::Tag(Tag::Deleted),
        DEFAULT if payload.is_empty() => Status::Tag(Tag::Default),
        STRIPPED if payload.is_empty() => Status::Tag(Tag::Stripped),
        MISSING if payload.is_empty() => Status::Tag(Tag::Missing),
        DELETED | DEFAULT | STRIPPED | MISSING => return Err("trailing bytes"),
        _ => return Err("invalid Tracked value tag"),
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::U256 as AlloyU256;

    use crate::crdt::{
        bytes::Bytes,
        int64::I64,
        state::{status::Tag, value::Values},
        u64_set::U64Set,
        u256::U256,
        uint64::U64,
    };

    use super::*;

    fn tracked(original: Status, current: Status) -> Tracked<Status, Status> {
        Tracked {
            id: 42,
            value: Values { original, current },
            operations: Operations {
                reads: 1.into(),
                existence_checks: 2.into(),
                removes: 3.into(),
                deltas: 4.into(),
            },
        }
    }

    fn assert_same(actual: &Tracked<Status, Status>, expected: &Tracked<Status, Status>) {
        assert_eq!(actual.id, expected.id);
        assert!(actual.value == expected.value);
        assert_eq!(actual.operations, expected.operations);
    }

    #[test]
    fn every_value_type_round_trips() {
        let values = vec![
            Status::Tag(Tag::Default),
            Status::Tag(Tag::Stripped),
            Status::Value(Bytes::new(vec![1, 2, 3]).unwrap().into()),
            Status::Value(I64::new(-10, 10).unwrap().into()),
            Status::Value(U64::new(0, 100).unwrap().into()),
            Status::Value(
                U256::new(AlloyU256::ZERO, AlloyU256::from(100))
                    .unwrap()
                    .into(),
            ),
            Status::Value(U64Set::new().unwrap().into()),
        ];

        for value in values {
            let expected = tracked(Status::Tag(Tag::Stripped), value);
            let encoded = encode(&expected).unwrap();
            assert_eq!(encoded.len() as u64, encoded_size(&expected).unwrap());
            assert_same(&decode(&encoded).unwrap(), &expected);
        }
    }

    #[test]
    fn marker_states_round_trip() {
        let deleted = tracked(Status::Tag(Tag::Stripped), Status::Tag(Tag::Deleted));
        let missing = tracked(Status::Tag(Tag::Missing), Status::Tag(Tag::Missing));

        assert_same(&decode(&encode(&deleted).unwrap()).unwrap(), &deleted);
        assert_same(&decode(&encode(&missing).unwrap()).unwrap(), &missing);
    }

    #[test]
    fn malformed_and_truncated_encodings_are_rejected() {
        let expected = tracked(
            Status::Tag(Tag::Missing),
            Status::Value(Bytes::new(vec![1, 2, 3]).unwrap().into()),
        );
        let encoded = encode(&expected).unwrap();
        for end in 0..encoded.len() {
            assert!(decode(&encoded[..end]).is_err());
        }

        let mut invalid_tag = encoded.clone();
        invalid_tag[0] = u8::MAX;
        assert_eq!(
            decode(&invalid_tag).err(),
            Some("invalid Tracked value tag")
        );

        let mut invalid_value_tag = encoded;
        invalid_value_tag[1] = u8::MAX;
        assert_eq!(
            decode(&invalid_value_tag).err(),
            Some("invalid Tracked value tag")
        );
    }

    #[test]
    fn short_buffer_is_rejected_without_writing() {
        let value = tracked(
            Status::Tag(Tag::Missing),
            Status::Value(Bytes::new(vec![1, 2, 3]).unwrap().into()),
        );
        let mut output = vec![0xaa; encoded_size(&value).unwrap() as usize - 1];

        assert_eq!(
            encode_to(&value, &mut output),
            Err("output buffer too small")
        );
        assert!(output.iter().all(|byte| *byte == 0xaa));
    }
}
