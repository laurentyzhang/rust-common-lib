use crate::crdt::bytes::Bytes;

use super::{Reader, Result, Writer};

const PRESENT: u8 = 1;

pub fn encoded_size(value: &Bytes) -> Result<u64> {
    let Some(bytes) = &value.delta else {
        return Ok(1);
    };

    let length = bytes.len() as u64;
    1u64.checked_add(8)
        .and_then(|size| size.checked_add(length))
        .ok_or("encoded size overflow")
}

pub fn encode(value: &Bytes) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &Bytes, output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(value)?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let mut writer = Writer::new(output);
    writer.write_u8(u8::from(value.delta.is_some()) * PRESENT)?;
    if let Some(bytes) = &value.delta {
        writer.write_u64(bytes.len() as u64)?;
        writer.write_bytes(bytes)?;
    }
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<Bytes> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !PRESENT != 0 {
        return Err("invalid Bytes flags");
    }

    let delta = if flags & PRESENT != 0 {
        let length =
            usize::try_from(reader.read_u64()?).map_err(|_| "byte length exceeds usize::MAX")?;
        Some(reader.read_bytes(length)?.to_vec().into_boxed_slice())
    } else {
        None
    };
    reader.finish()?;

    Ok(Bytes { delta })
}
