use alloy_primitives::U256 as AlloyU256;

use crate::crdt::u256::U256;

use super::{Reader, Result, Writer};

const VALUE: u8 = 1;
const DELTA: u8 = 2;
const LIMITS: u8 = 4;
// Existing additive encodings retain their original flags and layout.
const SUBTRACT: u8 = 8;

pub fn encoded_size(value: &U256) -> Result<u64> {
    Ok(97 + u64::from(value.delta.is_some()) * 32)
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
    let delta_flags = match value.delta {
        Some(crate::crdt::state::DeltaOp::Add(_)) => DELTA,
        Some(crate::crdt::state::DeltaOp::Sub(_)) => DELTA | SUBTRACT,
        None => 0,
    };
    writer.write_u8(VALUE | LIMITS | delta_flags)?;
    writer.write_bytes(&value.value.to_le_bytes::<32>())?;
    if let Some(delta) = &value.delta {
        let delta = match delta {
            crate::crdt::state::DeltaOp::Add(value) | crate::crdt::state::DeltaOp::Sub(value) => {
                value
            }
        };
        writer.write_bytes(&delta.to_le_bytes::<32>())?;
    }
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
        Some(if flags & SUBTRACT != 0 {
            crate::crdt::state::DeltaOp::Sub(AlloyU256::from_le_bytes(reader.read_array::<32>()?))
        } else {
            crate::crdt::state::DeltaOp::Add(AlloyU256::from_le_bytes(reader.read_array::<32>()?))
        })
    } else {
        None
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
        limits,
    })
}
