use crate::crdt::int64::I64;

use super::{Reader, Result, Writer};

const VALUE: u8 = 1;
const DELTA: u8 = 2;
const LIMITS: u8 = 4;

pub fn encoded_size(value: &I64) -> Result<u64> {
    Ok(1 + u64::from(u8::from(value.value.is_some())) * 8
        + u64::from(u8::from(value.delta.is_some())) * 8
        + u64::from(u8::from(value.limits.is_some())) * 16)
}

pub fn encode(value: &I64) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &I64, output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(value)?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let mut writer = Writer::new(output);
    writer.write_u8(
        u8::from(value.value.is_some()) * VALUE
            + u8::from(value.delta.is_some()) * DELTA
            + u8::from(value.limits.is_some()) * LIMITS,
    )?;
    if let Some(value) = value.value {
        writer.write_i64(value)?;
    }
    if let Some(delta) = value.delta {
        writer.write_i64(delta)?;
    }
    if let Some((lower, upper)) = value.limits {
        writer.write_i64(lower)?;
        writer.write_i64(upper)?;
    }
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<I64> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !(VALUE | DELTA | LIMITS) != 0 {
        return Err("invalid I64 flags");
    }

    let value = (flags & VALUE != 0)
        .then(|| reader.read_i64())
        .transpose()?;
    let delta = (flags & DELTA != 0)
        .then(|| reader.read_i64())
        .transpose()?;
    let limits = if flags & LIMITS != 0 {
        Some((reader.read_i64()?, reader.read_i64()?))
    } else {
        None
    };
    reader.finish()?;

    Ok(I64 {
        value,
        delta,
        limits,
    })
}
