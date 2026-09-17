use alloy_primitives::U256 as AlloyU256;

use crate::crdt::u256::U256;

use super::{Reader, Result, Writer};

const VALUE: u8 = 1;
const DELTA: u8 = 2;
const LIMITS: u8 = 4;
// Existing additive encodings retain their original flags and layout.
const SUBTRACT: u8 = 8;

pub fn encoded_size(_: &U256) -> Result<u64> {
    Ok(129)
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
    writer.write_u8(VALUE | DELTA | LIMITS | if value.delta_subtract { SUBTRACT } else { 0 })?;
    writer.write_bytes(&value.value.to_le_bytes::<32>())?;
    writer.write_bytes(&value.delta.to_le_bytes::<32>())?;
    writer.write_bytes(&value.limits.0.to_le_bytes::<32>())?;
    writer.write_bytes(&value.limits.1.to_le_bytes::<32>())?;
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<U256> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !(VALUE | DELTA | LIMITS | SUBTRACT) != 0
        || (flags & SUBTRACT != 0 && flags & DELTA == 0)
    {
        return Err("invalid U256 flags");
    }

    let value = if flags & VALUE != 0 {
        AlloyU256::from_le_bytes(reader.read_array::<32>()?)
    } else {
        AlloyU256::ZERO
    };
    let delta = if flags & DELTA != 0 {
        AlloyU256::from_le_bytes(reader.read_array::<32>()?)
    } else {
        AlloyU256::ZERO
    };
    let limits = if flags & LIMITS != 0 {
        (
            AlloyU256::from_le_bytes(reader.read_array::<32>()?),
            AlloyU256::from_le_bytes(reader.read_array::<32>()?),
        )
    } else {
        (AlloyU256::ZERO, AlloyU256::MAX)
    };
    reader.finish()?;

    Ok(U256 {
        value,
        delta,
        delta_subtract: flags & SUBTRACT != 0,
        limits,
    })
}
