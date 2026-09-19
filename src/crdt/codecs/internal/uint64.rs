use crate::crdt::uint64::U64;

use super::{Reader, Result, Writer};

const VALUE: u8 = 1;
const DELTA: u8 = 2;
const LIMITS: u8 = 4;
// Existing additive encodings retain their original flags and layout.
const SUBTRACT: u8 = 8;

pub fn encoded_size(value: &U64) -> Result<u64> {
    Ok(25 + u64::from(value.delta.is_some()) * 8)
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
    let delta_flags = match value.delta {
        Some(crate::crdt::state::DeltaOp::Add(_)) => DELTA,
        Some(crate::crdt::state::DeltaOp::Sub(_)) => DELTA | SUBTRACT,
        None => 0,
    };
    writer.write_u8(VALUE | LIMITS | delta_flags)?;
    writer.write_u64(value.value)?;
    if let Some(delta) = &value.delta {
        writer.write_u64(match delta {
            crate::crdt::state::DeltaOp::Add(value) | crate::crdt::state::DeltaOp::Sub(value) => {
                *value
            }
        })?;
    }
    writer.write_u64(value.limits.0)?;
    writer.write_u64(value.limits.1)?;
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<U64> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !(VALUE | DELTA | LIMITS | SUBTRACT) != 0
        || (flags & SUBTRACT != 0 && flags & DELTA == 0)
    {
        return Err("invalid U64 flags");
    }

    let value = if flags & VALUE != 0 {
        reader.read_u64()?
    } else {
        0
    };

    let delta = if flags & DELTA != 0 {
        Some(if flags & SUBTRACT != 0 {
            crate::crdt::state::DeltaOp::Sub(reader.read_u64()?)
        } else {
            crate::crdt::state::DeltaOp::Add(reader.read_u64()?)
        })
    } else {
        None
    };

    let limits = if flags & LIMITS != 0 {
        (reader.read_u64()?, reader.read_u64()?)
    } else {
        (u64::MIN, u64::MAX)
    };
    reader.finish()?;

    Ok(U64 {
        value,
        delta,
        limits,
    })
}
