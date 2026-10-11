use crate::crdt::NonComm;

use super::{Reader, Result, Writer};

const PRESENT: u8 = 1;

pub fn encoded_size<T: ?Sized + AsRef<[u8]>>(value: &NonComm<T>) -> Result<u64> {
    let Some(bytes) = value.delta.as_deref() else {
        return Ok(1);
    };

    let length = bytes.as_ref().len() as u64;
    1u64.checked_add(8)
        .and_then(|size| size.checked_add(length))
        .ok_or("encoded size overflow")
}

pub fn encode<T: ?Sized + AsRef<[u8]>>(value: &NonComm<T>) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to<T: ?Sized + AsRef<[u8]>>(value: &NonComm<T>, output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(value)?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let mut writer = Writer::new(output);
    writer.write_u8(u8::from(value.delta.is_some()) * PRESENT)?;
    if let Some(bytes) = value.delta.as_deref() {
        let bytes = bytes.as_ref();
        writer.write_u64(bytes.len() as u64)?;
        writer.write_bytes(bytes)?;
    }
    Ok(writer.finish() as u64)
}

pub fn decode<T: ?Sized>(input: &[u8]) -> Result<NonComm<T>>
where
    Box<T>: From<Vec<u8>>,
{
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !PRESENT != 0 {
        return Err("invalid Bytes flags");
    }

    let delta = if flags & PRESENT != 0 {
        let length =
            usize::try_from(reader.read_u64()?).map_err(|_| "byte length exceeds usize::MAX")?;
        Some(Box::<T>::from(reader.read_bytes(length)?.to_vec()))
    } else {
        None
    };
    reader.finish()?;

    Ok(NonComm { delta })
}
