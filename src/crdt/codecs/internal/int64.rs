use crate::crdt::int64::I64;

use super::{Reader, Result, Writer};

const VALUE: u8 = 1;
const DELTA: u8 = 2;
const LIMITS: u8 = 4;

pub fn encoded_size(_: &I64) -> Result<u64> {
    Ok(33)
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
    writer.write_u8(VALUE | DELTA | LIMITS)?;
    writer.write_i64(value.value)?;
    writer.write_i64(value.delta)?;
    writer.write_i64(value.limits.0)?;
    writer.write_i64(value.limits.1)?;
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<I64> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !(VALUE | DELTA | LIMITS) != 0 {
        return Err("invalid I64 flags");
    }

    let value = if flags & VALUE != 0 {
        reader.read_i64()?
    } else {
        0
    };
    let delta = if flags & DELTA != 0 {
        reader.read_i64()?
    } else {
        0
    };
    let limits = if flags & LIMITS != 0 {
        (reader.read_i64()?, reader.read_i64()?)
    } else {
        (i64::MIN, i64::MAX)
    };
    reader.finish()?;

    Ok(I64 {
        value,
        delta,
        limits,
    })
}
