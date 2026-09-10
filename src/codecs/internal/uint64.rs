use crate::crdt::uint64::U64;

use super::{Reader, Result, Writer};

const VALUE: u8 = 1;
const DELTA: u8 = 2;
const LIMITS: u8 = 4;

pub fn encoded_size(value: &U64) -> Result<u64> {
    Ok(1 + u64::from(u8::from(value.value.is_some())) * 8
        + u64::from(u8::from(value.delta.is_some())) * 8
        + u64::from(u8::from(value.limits.is_some())) * 16)
}

pub fn encode(value: &U64) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &U64, output: &mut [u8]) -> Result<u64> {
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
        writer.write_u64(value)?;
    }
    if let Some(delta) = value.delta {
        writer.write_u64(delta)?;
    }
    if let Some((lower, upper)) = value.limits {
        writer.write_u64(lower)?;
        writer.write_u64(upper)?;
    }
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<U64> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !(VALUE | DELTA | LIMITS) != 0 {
        return Err("invalid U64 flags");
    }

    let value = (flags & VALUE != 0)
        .then(|| reader.read_u64())
        .transpose()?;
    let delta = (flags & DELTA != 0)
        .then(|| reader.read_u64())
        .transpose()?;
    let limits = if flags & LIMITS != 0 {
        Some((reader.read_u64()?, reader.read_u64()?))
    } else {
        None
    };
    reader.finish()?;

    Ok(U64 {
        value,
        delta,
        limits,
    })
}
