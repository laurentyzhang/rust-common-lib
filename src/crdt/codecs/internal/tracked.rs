use std::borrow::Cow;

use crate::crdt::state::{Numeric, Tracked, Value};

use super::{Reader, Result, Writer, bytes, int64, u64_set, u256, uint64};

const NONE: u8 = 0;
const BYTES: u8 = 1;
const I64: u8 = 2;
const U64: u8 = 3;
const U256: u8 = 4;
const U64_SET: u8 = 5;

const IS_NEW: u8 = 1;
const TOMBSTONE: u8 = 2;
const HEADER_SIZE: u64 = 22;

fn value_tag(value: &Value<'_>) -> u8 {
    match value {
        Value::None => NONE,
        Value::Bytes(_) => BYTES,
        Value::Numeric(Numeric::I64(_)) => I64,
        Value::Numeric(Numeric::U64(_)) => U64,
        Value::Numeric(Numeric::U256(_)) => U256,
        Value::U64Set(_) => U64_SET,
    }
}

fn value_size(value: &Value<'_>) -> Result<u64> {
    match value {
        Value::None => Ok(0),
        Value::Bytes(value) => bytes::encoded_size(value),
        Value::Numeric(Numeric::I64(value)) => int64::encoded_size(value),
        Value::Numeric(Numeric::U64(value)) => uint64::encoded_size(value),
        Value::Numeric(Numeric::U256(value)) => u256::encoded_size(value),
        Value::U64Set(value) => u64_set::encoded_size(value),
    }
}

fn validate(value: &Tracked<'_>) -> Result<()> {
    if value.tombstone && matches!(value.value, Value::None) {
        return Err("tombstone must retain a value");
    }
    Ok(())
}

pub fn encoded_size(value: &Tracked<'_>) -> Result<u64> {
    validate(value)?;
    HEADER_SIZE
        .checked_add(value_size(&value.value)?)
        .ok_or("encoded size overflow")
}

pub fn encode(value: &Tracked<'_>) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &Tracked<'_>, output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(value)?;
    let size_usize = usize::try_from(size).map_err(|_| "encoded size exceeds usize::MAX")?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let mut writer = Writer::new(output);
    writer.write_u8(value_tag(&value.value))?;
    writer.write_u8(u8::from(value.is_new) * IS_NEW | u8::from(value.tombstone) * TOMBSTONE)?;
    writer.write_u32(value.reads)?;
    writer.write_u32(value.checks)?;
    writer.write_u32(value.writes)?;
    writer.write_u32(value.deltas)?;
    writer.write_u32(value.creates)?;
    let header_size = writer.finish();

    let payload = &mut output[header_size..size_usize];
    let written = match &value.value {
        Value::None => 0,
        Value::Bytes(value) => bytes::encode_to(value, payload)?,
        Value::Numeric(Numeric::I64(value)) => int64::encode_to(value, payload)?,
        Value::Numeric(Numeric::U64(value)) => uint64::encode_to(value, payload)?,
        Value::Numeric(Numeric::U256(value)) => u256::encode_to(value, payload)?,
        Value::U64Set(value) => u64_set::encode_to(value, payload)?,
    };
    if HEADER_SIZE + written != size {
        return Err("encoded size mismatch");
    }
    Ok(size)
}

pub fn decode(input: &[u8]) -> Result<Tracked<'static>> {
    let mut reader = Reader::new(input);
    let tag = reader.read_u8()?;
    let flags = reader.read_u8()?;
    if flags & !(IS_NEW | TOMBSTONE) != 0 {
        return Err("invalid Tracked flags");
    }

    let reads = reader.read_u32()?;
    let checks = reader.read_u32()?;
    let writes = reader.read_u32()?;
    let deltas = reader.read_u32()?;
    let creates = reader.read_u32()?;
    let payload = reader.read_bytes(reader.remaining())?;
    reader.finish()?;

    let value = match tag {
        NONE if payload.is_empty() => Value::None,
        NONE => return Err("trailing bytes"),
        BYTES => Value::Bytes(Cow::Owned(bytes::decode(payload)?)),
        I64 => Value::Numeric(Numeric::I64(Cow::Owned(int64::decode(payload)?))),
        U64 => Value::Numeric(Numeric::U64(Cow::Owned(uint64::decode(payload)?))),
        U256 => Value::Numeric(Numeric::U256(Cow::Owned(u256::decode(payload)?))),
        U64_SET => Value::U64Set(Cow::Owned(u64_set::decode(payload)?)),
        _ => return Err("invalid Tracked value tag"),
    };

    let tracked = Tracked {
        value,
        reads,
        checks,
        writes,
        deltas,
        creates,
        is_new: flags & IS_NEW != 0,
        tombstone: flags & TOMBSTONE != 0,
    };
    validate(&tracked)?;
    Ok(tracked)
}

#[cfg(test)]
mod tests {
    use alloy_primitives::U256 as AlloyU256;

    use crate::crdt::{bytes::Bytes, int64::I64, u64_set::U64Set, u256::U256, uint64::U64};

    use super::*;

    fn tracked(value: Value<'static>) -> Tracked<'static> {
        Tracked {
            value,
            reads: 1,
            checks: 2,
            writes: 3,
            deltas: 4,
            creates: 5,
            is_new: true,
            tombstone: false,
        }
    }

    fn assert_same(actual: &Tracked<'_>, expected: &Tracked<'_>) {
        assert!(actual.value == expected.value);
        assert_eq!(actual.reads, expected.reads);
        assert_eq!(actual.checks, expected.checks);
        assert_eq!(actual.writes, expected.writes);
        assert_eq!(actual.deltas, expected.deltas);
        assert_eq!(actual.creates, expected.creates);
        assert_eq!(actual.is_new, expected.is_new);
        assert_eq!(actual.tombstone, expected.tombstone);
    }

    #[test]
    fn every_value_type_round_trips() {
        let values = vec![
            Value::None,
            Bytes::new(vec![1, 2, 3]).unwrap().into(),
            I64::new(-10, 10).unwrap().into(),
            U64::new(0, 100).unwrap().into(),
            U256::new(AlloyU256::ZERO, AlloyU256::from(100))
                .unwrap()
                .into(),
            U64Set::new().unwrap().into(),
        ];

        for value in values {
            let expected = tracked(value);
            let encoded = encode(&expected).unwrap();
            assert_eq!(encoded.len() as u64, encoded_size(&expected).unwrap());
            assert_same(&decode(&encoded).unwrap(), &expected);
        }
    }

    #[test]
    fn tombstone_round_trips_with_retained_value() {
        let mut expected = tracked(U64::new(0, 100).unwrap().into());
        expected.tombstone = true;

        assert_same(&decode(&encode(&expected).unwrap()).unwrap(), &expected);
    }

    #[test]
    fn malformed_and_truncated_encodings_are_rejected() {
        let expected = tracked(Bytes::new(vec![1, 2, 3]).unwrap().into());
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

        let mut invalid_flags = encoded;
        invalid_flags[1] = 4;
        assert_eq!(decode(&invalid_flags).err(), Some("invalid Tracked flags"));
    }

    #[test]
    fn short_buffer_is_rejected_without_writing() {
        let value = tracked(Bytes::new(vec![1, 2, 3]).unwrap().into());
        let mut output = vec![0xaa; encoded_size(&value).unwrap() as usize - 1];

        assert_eq!(
            encode_to(&value, &mut output),
            Err("output buffer too small")
        );
        assert!(output.iter().all(|byte| *byte == 0xaa));
    }
}
