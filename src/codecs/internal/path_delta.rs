use crate::crdt::path_meta::PathDelta;

use super::{Reader, Result, Writer};

pub fn encoded_size(value: &PathDelta) -> Result<u64> {
    let added = value.added.len() as u64;
    let removed = value.removed.len() as u64;

    added
        .checked_add(removed)
        .and_then(|count| count.checked_mul(8))
        .and_then(|size| size.checked_add(16))
        .ok_or("encoded size overflow")
}

pub fn encode(value: &PathDelta) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &PathDelta, output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(value)?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let mut writer = Writer::new(output);
    write(value, &mut writer)?;
    Ok(writer.finish() as u64)
}

pub fn decode(input: &[u8]) -> Result<PathDelta> {
    let mut reader = Reader::new(input);
    let value = read(&mut reader)?;
    reader.finish()?;
    Ok(value)
}

pub(super) fn write(value: &PathDelta, writer: &mut Writer<'_>) -> Result<()> {
    writer.write_u64(value.added.len() as u64)?;
    for entry in &value.added {
        writer.write_u64(*entry)?;
    }

    writer.write_u64(value.removed.len() as u64)?;
    for entry in &value.removed {
        writer.write_u64(*entry)?;
    }
    Ok(())
}

pub(super) fn read(reader: &mut Reader<'_>) -> Result<PathDelta> {
    let added_count =
        usize::try_from(reader.read_u64()?).map_err(|_| "added count exceeds usize::MAX")?;
    if added_count > reader.remaining().saturating_sub(8) / 8 {
        return Err("invalid added count");
    }
    let mut added = Vec::new();
    added
        .try_reserve_exact(added_count)
        .map_err(|_| "added allocation failed")?;
    for _ in 0..added_count {
        added.push(reader.read_u64()?);
    }

    let removed_count =
        usize::try_from(reader.read_u64()?).map_err(|_| "removed count exceeds usize::MAX")?;
    if removed_count > reader.remaining() / 8 {
        return Err("invalid removed count");
    }
    let mut removed = Vec::new();
    removed
        .try_reserve_exact(removed_count)
        .map_err(|_| "removed allocation failed")?;
    for _ in 0..removed_count {
        removed.push(reader.read_u64()?);
    }

    Ok(PathDelta { added, removed })
}
