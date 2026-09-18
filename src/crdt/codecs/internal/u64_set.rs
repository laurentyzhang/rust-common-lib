use crate::{collections::delta_set::DeltaSet, crdt::u64_set::U64Set};

use super::{Reader, Result, Writer, delta_op};

const DELTA: u8 = 1;

pub fn encoded_size(value: &U64Set) -> Result<u64> {
    let slots = value.entries.slots();
    let bitmap_size = slots.len().div_ceil(8) as u64;
    let live_size = slots
        .iter()
        .filter(|slot| slot.is_some())
        .try_fold(0u64, |size, _| {
            size.checked_add(8).ok_or("encoded size overflow")
        })?;
    let mut size = 9u64
        .checked_add(bitmap_size)
        .and_then(|size| size.checked_add(live_size))
        .ok_or("encoded size overflow")?;

    if let Some(delta) = &value.delta {
        size = size
            .checked_add(delta_op::encoded_size(delta)?)
            .ok_or("encoded size overflow")?;
    }
    Ok(size)
}

pub fn encode(value: &U64Set) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(value)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(value, &mut output)?;
    Ok(output)
}

pub fn encode_to(value: &U64Set, output: &mut [u8]) -> Result<u64> {
    let size = encoded_size(value)?;
    if (output.len() as u64) < size {
        return Err("output buffer too small");
    }

    let slots = value.entries.slots();
    let bitmap_size = slots.len().div_ceil(8);
    let (header, remaining) = output.split_at_mut(9);
    header[0] = u8::from(value.delta.is_some()) * DELTA;
    header[1..].copy_from_slice(&(slots.len() as u64).to_le_bytes());

    let (bitmap, payload) = remaining.split_at_mut(bitmap_size);
    bitmap.fill(0);
    let mut writer = Writer::new(payload);
    for (index, slot) in slots.iter().enumerate() {
        if let Some(entry) = slot {
            bitmap[index / 8] |= 1 << (index % 8);
            writer.write_u64(*entry)?;
        }
    }

    if let Some(delta) = &value.delta {
        delta_op::write(delta, &mut writer)?;
    }

    let written = 9u64
        .checked_add(bitmap_size as u64)
        .and_then(|written| written.checked_add(writer.finish() as u64))
        .ok_or("encoded size overflow")?;
    if written != size {
        return Err("encoded size mismatch");
    }
    Ok(written)
}

pub fn decode(input: &[u8]) -> Result<U64Set> {
    let mut reader = Reader::new(input);
    let flags = reader.read_u8()?;
    if flags & !DELTA != 0 {
        return Err("invalid U64Set flags");
    }

    let slot_count =
        usize::try_from(reader.read_u64()?).map_err(|_| "slot count exceeds usize::MAX")?;
    let bitmap_size = slot_count.div_ceil(8);
    let bitmap = reader.read_bytes(bitmap_size)?;

    let remainder = slot_count % 8;
    if remainder != 0 {
        let valid_bits = (1u8 << remainder) - 1;
        if bitmap.last().copied().unwrap_or(0) & !valid_bits != 0 {
            return Err("invalid U64Set bitmap");
        }
    }

    let live_count = bitmap.iter().try_fold(0usize, |count, byte| {
        count
            .checked_add(byte.count_ones() as usize)
            .ok_or("live slot count overflow")
    })?;
    let live_size = live_count.checked_mul(8).ok_or("live slot size overflow")?;
    if live_size > reader.remaining() {
        return Err("invalid live slot count");
    }

    let mut slots = Vec::new();
    slots
        .try_reserve_exact(slot_count)
        .map_err(|_| "slot allocation failed")?;
    for index in 0..slot_count {
        if bitmap[index / 8] & (1 << (index % 8)) == 0 {
            slots.push(None);
        } else {
            slots.push(Some(reader.read_u64()?));
        }
    }

    let delta = if flags & DELTA != 0 {
        Some(delta_op::read(&mut reader)?)
    } else {
        None
    };
    reader.finish()?;

    Ok(U64Set {
        entries: DeltaSet::try_from_slots(slots).ok_or("duplicate or unindexable U64Set entry")?,
        delta,
    })
}
