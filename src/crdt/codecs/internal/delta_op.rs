use crate::crdt::state::DeltaOp;

use super::{Reader, Result, Writer};

pub fn encoded_size(value: &[DeltaOp<u64>]) -> Result<u64> {
    (value.len() as u64)
        .checked_mul(8)
        .and_then(|size| size.checked_add(16))
        .ok_or("encoded size overflow")
}

pub fn encode(value: &[DeltaOp<u64>]) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &[DeltaOp<u64>], output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(value)?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let mut writer = Writer::new(output);
    write(value, &mut writer)?;
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<Vec<DeltaOp<u64>>> {
    let mut reader = Reader::new(input);
    let value = read(&mut reader)?;
    reader.finish()?;
    Ok(value)
}

pub(super) fn write(value: &[DeltaOp<u64>], writer: &mut Writer<'_>) -> Result<()> {
    let added_count = value
        .iter()
        .filter(|operation| matches!(operation, DeltaOp::Add(_)))
        .count() as u64;
    writer.write_u64(added_count)?;
    for operation in value {
        if let DeltaOp::Add(entry) = operation {
            writer.write_u64(*entry)?;
        }
    }

    let removed_count = value.len() as u64 - added_count;
    writer.write_u64(removed_count)?;
    for operation in value {
        if let DeltaOp::Sub(entry) = operation {
            writer.write_u64(*entry)?;
        }
    }
    Ok(())
}

pub(super) fn read(reader: &mut Reader<'_>) -> Result<Vec<DeltaOp<u64>>> {
    let added_count =
        usize::try_from(reader.read_u64()?).map_err(|_| "added count exceeds usize::MAX")?;
    if added_count > reader.remaining().saturating_sub(8) / 8 {
        return Err("invalid added count");
    }
    let mut operations = Vec::new();
    operations
        .try_reserve_exact(added_count)
        .map_err(|_| "added allocation failed")?;
    for _ in 0..added_count {
        operations.push(DeltaOp::Add(reader.read_u64()?));
    }

    let removed_count =
        usize::try_from(reader.read_u64()?).map_err(|_| "removed count exceeds usize::MAX")?;
    if removed_count > reader.remaining() / 8 {
        return Err("invalid removed count");
    }
    operations
        .try_reserve_exact(removed_count)
        .map_err(|_| "removed allocation failed")?;
    for _ in 0..removed_count {
        operations.push(DeltaOp::Sub(reader.read_u64()?));
    }

    Ok(operations)
}
