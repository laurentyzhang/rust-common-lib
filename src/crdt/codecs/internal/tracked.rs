use std::borrow::Cow;

use crate::crdt::state::{Marker, Numeric, Tracked, Value};

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

fn value_tag(value: &Value<'_>) -> u8 {
    match value {
        Value::Bytes(_) => BYTES,
        Value::Numeric(Numeric::I64(_)) => I64,
        Value::Numeric(Numeric::U64(_)) => U64,
        Value::Numeric(Numeric::U256(_)) => U256,
        Value::U64Set(_) => U64_SET,
        Value::Marker(Marker::Deleted) => DELETED,
        Value::Marker(Marker::Default) => DEFAULT,
        Value::Marker(Marker::Stripped) => STRIPPED,
        Value::Marker(Marker::Missing) => MISSING,
    }
}

fn value_size(value: &Value<'_>) -> Result<u64> {
    match value {
        Value::Marker(_) => Ok(0),
        Value::Bytes(value) => bytes::encoded_size(value),
        Value::Numeric(Numeric::I64(value)) => int64::encoded_size(value),
        Value::Numeric(Numeric::U64(value)) => uint64::encoded_size(value),
        Value::Numeric(Numeric::U256(value)) => u256::encoded_size(value),
        Value::U64Set(value) => u64_set::encoded_size(value),
    }
}

fn encode_value_to(value: &Value<'_>, output: &mut [u8]) -> Result<u64> {
    match value {
        Value::Marker(_) => Ok(0),
        Value::Bytes(value) => bytes::encode_to(value, output),
        Value::Numeric(Numeric::I64(value)) => int64::encode_to(value, output),
        Value::Numeric(Numeric::U64(value)) => uint64::encode_to(value, output),
        Value::Numeric(Numeric::U256(value)) => u256::encode_to(value, output),
        Value::U64Set(value) => u64_set::encode_to(value, output),
    }
}

pub fn encoded_size(tracked: &Tracked<Value<'_>>) -> Result<u64> {
    let original_size = value_size(&tracked.original)?;
    let current_size = value_size(&tracked.current)?;
    HEADER_SIZE
        .checked_add(original_size)
        .and_then(|size| size.checked_add(current_size))
        .ok_or("encoded size overflow")
}

pub fn encode(tracked: &Tracked<Value<'_>>) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(tracked)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(tracked, &mut output)?;
    Ok(output)
}

pub fn encode_to(tracked: &Tracked<Value<'_>>, output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(tracked)?;
    let size_usize = usize::try_from(size).map_err(|_| "encoded size exceeds usize::MAX")?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let mut writer = Writer::new(output);
    let original_size = value_size(&tracked.original)?;
    writer.write_u8(value_tag(&tracked.original))?;
    writer.write_u8(value_tag(&tracked.current))?;
    writer.write_u64(original_size)?;
    writer.write_u64(tracked.id)?;
    writer.write_u32(tracked.reads.count())?;
    writer.write_u32(tracked.existence_checks.count())?;
    writer.write_u32(tracked.writes.count())?;
    writer.write_u32(tracked.deltas.count())?;
    let header_size = writer.finish();

    let payload = &mut output[header_size..size_usize];
    let original_size_usize =
        usize::try_from(original_size).map_err(|_| "encoded size exceeds usize::MAX")?;
    let (original_payload, current_payload) = payload.split_at_mut(original_size_usize);
    let original_written = encode_value_to(&tracked.original, original_payload)?;
    let current_written = encode_value_to(&tracked.current, current_payload)?;
    if HEADER_SIZE + original_written + current_written != size {
        return Err("encoded size mismatch");
    }
    Ok(size)
}

pub fn decode(input: &[u8]) -> Result<Tracked<Value<'static>>> {
    let mut reader = Reader::new(input);
    let original_tag = reader.read_u8()?;
    let current_tag = reader.read_u8()?;
    let original_size = reader.read_u64()?;
    let id = reader.read_u64()?;
    let reads = reader.read_u32()?;
    let existence_checks = reader.read_u32()?;
    let writes = reader.read_u32()?;
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
        original,
        current,
        reads: reads.into(),
        existence_checks: existence_checks.into(),
        writes: writes.into(),
        deltas: deltas.into(),
    };
    Ok(tracked)
}

fn decode_value(tag: u8, payload: &[u8]) -> Result<Value<'static>> {
    let value = match tag {
        BYTES => Value::Bytes(Cow::Owned(bytes::decode(payload)?)),
        I64 => Value::Numeric(Numeric::I64(Cow::Owned(int64::decode(payload)?))),
        U64 => Value::Numeric(Numeric::U64(Cow::Owned(uint64::decode(payload)?))),
        U256 => Value::Numeric(Numeric::U256(Cow::Owned(u256::decode(payload)?))),
        U64_SET => Value::U64Set(Cow::Owned(u64_set::decode(payload)?)),
        DELETED if payload.is_empty() => Value::Marker(Marker::Deleted),
        DEFAULT if payload.is_empty() => Value::Marker(Marker::Default),
        STRIPPED if payload.is_empty() => Value::Marker(Marker::Stripped),
        MISSING if payload.is_empty() => Value::Marker(Marker::Missing),
        DELETED | DEFAULT | STRIPPED | MISSING => return Err("trailing bytes"),
        _ => return Err("invalid Tracked value tag"),
    };
    Ok(value)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::U256 as AlloyU256;

    use crate::crdt::{bytes::Bytes, int64::I64, u64_set::U64Set, u256::U256, uint64::U64};

    use super::*;

    fn tracked(original: Value<'static>, current: Value<'static>) -> Tracked<Value<'static>> {
        Tracked {
            id: 42,
            original,
            current,
            reads: 1.into(),
            existence_checks: 2.into(),
            writes: 3.into(),
            deltas: 4.into(),
        }
    }

    fn assert_same(actual: &Tracked<Value<'_>>, expected: &Tracked<Value<'_>>) {
        assert_eq!(actual.id, expected.id);
        assert!(actual.original == expected.original);
        assert!(actual.current == expected.current);
        assert_eq!(actual.reads, expected.reads);
        assert_eq!(actual.existence_checks, expected.existence_checks);
        assert_eq!(actual.writes, expected.writes);
        assert_eq!(actual.deltas, expected.deltas);
    }

    #[test]
    fn every_value_type_round_trips() {
        let values = vec![
            Value::Marker(Marker::Default),
            Value::Marker(Marker::Stripped),
            Bytes::new(vec![1, 2, 3]).unwrap().into(),
            I64::new(-10, 10).unwrap().into(),
            U64::new(0, 100).unwrap().into(),
            U256::new(AlloyU256::ZERO, AlloyU256::from(100))
                .unwrap()
                .into(),
            U64Set::new().unwrap().into(),
        ];

        for value in values {
            let expected = tracked(Value::Marker(Marker::Stripped), value);
            let encoded = encode(&expected).unwrap();
            assert_eq!(encoded.len() as u64, encoded_size(&expected).unwrap());
            assert_same(&decode(&encoded).unwrap(), &expected);
        }
    }

    #[test]
    fn marker_states_round_trip() {
        let deleted = tracked(
            Value::Marker(Marker::Stripped),
            Value::Marker(Marker::Deleted),
        );
        let missing = tracked(
            Value::Marker(Marker::Missing),
            Value::Marker(Marker::Missing),
        );

        assert_same(&decode(&encode(&deleted).unwrap()).unwrap(), &deleted);
        assert_same(&decode(&encode(&missing).unwrap()).unwrap(), &missing);
    }

    #[test]
    fn malformed_and_truncated_encodings_are_rejected() {
        let expected = tracked(
            Value::Marker(Marker::Missing),
            Bytes::new(vec![1, 2, 3]).unwrap().into(),
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
            Value::Marker(Marker::Missing),
            Bytes::new(vec![1, 2, 3]).unwrap().into(),
        );
        let mut output = vec![0xaa; encoded_size(&value).unwrap() as usize - 1];

        assert_eq!(
            encode_to(&value, &mut output),
            Err("output buffer too small")
        );
        assert!(output.iter().all(|byte| *byte == 0xaa));
    }
}
