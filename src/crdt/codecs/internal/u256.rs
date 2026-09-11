use alloy_primitives::U256 as AlloyU256;

use crate::crdt::u256::U256;

use super::{Reader, Result, Writer};

const VALUE: u8 = 1;
const DELTA: u8 = 2;
const LIMITS: u8 = 4;

pub fn encoded_size(value: &U256) -> Result<u64> {
    Ok(1 + u64::from(u8::from(value.value.is_some())) * 32
        + u64::from(u8::from(value.delta.is_some())) * 32
        + u64::from(u8::from(value.limits.is_some())) * 64)
}

pub fn encode(value: &U256) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &U256, output: &mut [u8]) -> Result<u64> {
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
        writer.write_bytes(&value.to_le_bytes::<32>())?;
    }
    if let Some(delta) = value.delta {
        writer.write_bytes(&delta.to_le_bytes::<32>())?;
    }
    if let Some((lower, upper)) = value.limits {
        writer.write_bytes(&lower.to_le_bytes::<32>())?;
        writer.write_bytes(&upper.to_le_bytes::<32>())?;
    }
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<U256> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !(VALUE | DELTA | LIMITS) != 0 {
        return Err("invalid U256 flags");
    }

    let value = (flags & VALUE != 0)
        .then(|| reader.read_array::<32>().map(AlloyU256::from_le_bytes))
        .transpose()?;
    let delta = (flags & DELTA != 0)
        .then(|| reader.read_array::<32>().map(AlloyU256::from_le_bytes))
        .transpose()?;
    let limits = if flags & LIMITS != 0 {
        Some((
            AlloyU256::from_le_bytes(reader.read_array::<32>()?),
            AlloyU256::from_le_bytes(reader.read_array::<32>()?),
        ))
    } else {
        None
    };
    reader.finish()?;

    Ok(U256 {
        value,
        delta,
        limits,
    })
}
