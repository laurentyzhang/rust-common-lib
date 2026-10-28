use alloy_primitives::{I256, U256};

use crate::crdt::Scalar;

use super::{Reader, Result, Writer};

const VALUE: u8 = 1;
const DELTA: u8 = 2;
const LIMITS: u8 = 4;
const SUBTRACT: u8 = 8;

pub fn encoded_size(value: &Scalar<I256>) -> Result<u64> {
    Ok(97 + u64::from(value.delta.is_some()) * 32)
}

pub fn encode(value: &Scalar<I256>) -> Result<Vec<u8>> {
    let size = usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &Scalar<I256>, output: &mut [u8]) -> Result<u64> {
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
    writer.write_bytes(&value.value.into_raw().to_le_bytes::<32>())?;
    if let Some(delta) = &value.delta {
        let magnitude = match delta {
            crate::crdt::state::DeltaOp::Add(value)
            | crate::crdt::state::DeltaOp::Sub(value) => value,
        };
        writer.write_bytes(&magnitude.to_le_bytes::<32>())?;
    }
    writer.write_bytes(&value.min.into_raw().to_le_bytes::<32>())?;
    writer.write_bytes(&value.max.into_raw().to_le_bytes::<32>())?;
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<Scalar<I256>> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !(VALUE | DELTA | LIMITS | SUBTRACT) != 0
        || (flags & SUBTRACT != 0 && flags & DELTA == 0)
    {
        return Err("invalid I256 flags");
    }

    let value = if flags & VALUE != 0 {
        I256::from_raw(U256::from_le_bytes(reader.read_array::<32>()?))
    } else {
        I256::ZERO
    };
    let delta = if flags & DELTA != 0 {
        let magnitude = U256::from_le_bytes(reader.read_array::<32>()?);
        Some(if flags & SUBTRACT != 0 {
            crate::crdt::state::DeltaOp::Sub(magnitude)
        } else {
            crate::crdt::state::DeltaOp::Add(magnitude)
        })
    } else {
        None
    };
    let (min, max) = if flags & LIMITS != 0 {
        (
            I256::from_raw(U256::from_le_bytes(reader.read_array::<32>()?)),
            I256::from_raw(U256::from_le_bytes(reader.read_array::<32>()?)),
        )
    } else {
        (I256::MIN, I256::MAX)
    };
    reader.finish()?;

    Ok(Scalar { value, delta, min, max })
}
