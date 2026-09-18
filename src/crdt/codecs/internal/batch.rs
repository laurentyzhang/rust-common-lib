use crate::crdt::{
    bytes::Bytes,
    int64::I64,
    state::{DeltaOp, Tracked},
    u64_set::U64Set,
    u256::U256,
    uint64::U64,
};

use super::{Result, Writer, bytes, delta_op, int64, tracked, u64_set, u256, uint64};

const PARALLEL_THRESHOLD: usize = 2_048;

pub trait InternalEncode: Sync {
    fn encoded_size(&self) -> Result<u64>;
    fn encode_to(&self, output: &mut [u8]) -> Result<u64>;
}

impl InternalEncode for Bytes {
    fn encoded_size(&self) -> Result<u64> {
        bytes::encoded_size(self)
    }

    fn encode_to(&self, output: &mut [u8]) -> Result<u64> {
        bytes::encode_to(self, output)
    }
}

impl InternalEncode for I64 {
    fn encoded_size(&self) -> Result<u64> {
        int64::encoded_size(self)
    }

    fn encode_to(&self, output: &mut [u8]) -> Result<u64> {
        int64::encode_to(self, output)
    }
}

impl InternalEncode for U64 {
    fn encoded_size(&self) -> Result<u64> {
        uint64::encoded_size(self)
    }

    fn encode_to(&self, output: &mut [u8]) -> Result<u64> {
        uint64::encode_to(self, output)
    }
}

impl InternalEncode for U256 {
    fn encoded_size(&self) -> Result<u64> {
        u256::encoded_size(self)
    }

    fn encode_to(&self, output: &mut [u8]) -> Result<u64> {
        u256::encode_to(self, output)
    }
}

impl InternalEncode for Vec<DeltaOp<u64>> {
    fn encoded_size(&self) -> Result<u64> {
        delta_op::encoded_size(self)
    }

    fn encode_to(&self, output: &mut [u8]) -> Result<u64> {
        delta_op::encode_to(self, output)
    }
}

impl InternalEncode for U64Set {
    fn encoded_size(&self) -> Result<u64> {
        u64_set::encoded_size(self)
    }

    fn encode_to(&self, output: &mut [u8]) -> Result<u64> {
        u64_set::encode_to(self, output)
    }
}

impl InternalEncode for Tracked<'_> {
    fn encoded_size(&self) -> Result<u64> {
        tracked::encoded_size(self)
    }

    fn encode_to(&self, output: &mut [u8]) -> Result<u64> {
        tracked::encode_to(self, output)
    }
}

/// Returns the batch size including its count and item-offset header.
pub fn encoded_size(values: &[&dyn InternalEncode]) -> Result<u64> {
    let header_size = (values.len() as u64)
        .checked_add(1)
        .and_then(|count| count.checked_mul(8))
        .ok_or("batch header size overflow")?;

    values.iter().try_fold(header_size, |size, value| {
        size.checked_add(value.encoded_size()?)
            .ok_or("batch encoded size overflow")
    })
}

/// Allocates one exact-size buffer and encodes every item into it.
pub fn encode(values: &[&dyn InternalEncode]) -> Result<Vec<u8>> {
    let size =
        usize::try_from(encoded_size(values)?).map_err(|_| "encoded size exceeds usize::MAX")?;
    let mut output = vec![0; size];
    encode_to(values, &mut output)?;
    Ok(output)
}

/// Encodes a count, payload-relative item offsets, and all item payloads.
///
/// Batches of at least 2,048 items encode their non-overlapping payload
/// sections in parallel.
pub fn encode_to(values: &[&dyn InternalEncode], output: &mut [u8]) -> Result<u64> {
    let sizes = values
        .iter()
        .map(|value| value.encoded_size())
        .collect::<Result<Vec<_>>>()?;
    let header_size_u64 = (values.len() as u64)
        .checked_add(1)
        .and_then(|count| count.checked_mul(8))
        .ok_or("batch header size overflow")?;
    let total_size = sizes.iter().try_fold(header_size_u64, |total, size| {
        total
            .checked_add(*size)
            .ok_or("batch encoded size overflow")
    })?;
    if (output.len() as u64) < total_size {
        return Err("output buffer too small");
    }

    let header_size = header_size_u64 as usize;
    let (header, payload) = output.split_at_mut(header_size);

    let mut writer = Writer::new(header);
    writer.write_u64(values.len() as u64)?;
    let mut offset = 0u64;
    for size in &sizes {
        writer.write_u64(offset)?;
        offset = offset
            .checked_add(*size)
            .ok_or("batch encoded size overflow")?;
    }

    encode_payload(values, &sizes, payload)?;
    Ok(total_size)
}

fn encode_payload(values: &[&dyn InternalEncode], sizes: &[u64], output: &mut [u8]) -> Result<()> {
    if values.len() < PARALLEL_THRESHOLD {
        let mut remaining = output;
        for (value, size) in values.iter().zip(sizes) {
            let size_usize =
                usize::try_from(*size).map_err(|_| "encoded size exceeds usize::MAX")?;
            let (section, rest) = remaining.split_at_mut(size_usize);
            if value.encode_to(section)? != *size {
                return Err("encoded size mismatch");
            }
            remaining = rest;
        }
        return Ok(());
    }

    let middle = values.len() / 2;
    let left_size = sizes[..middle].iter().try_fold(0u64, |total, size| {
        total
            .checked_add(*size)
            .ok_or("batch encoded size overflow")
    })?;
    let left_size = usize::try_from(left_size).map_err(|_| "batch section exceeds usize::MAX")?;
    let (left_output, right_output) = output.split_at_mut(left_size);
    let (left, right) = rayon::join(
        || encode_payload(&values[..middle], &sizes[..middle], left_output),
        || encode_payload(&values[middle..], &sizes[middle..], right_output),
    );
    left?;
    right
}
