use alloy_rlp::Encodable;

/// Returns the exact number of bytes needed to RLP-encode `value`.
pub fn encoded_size<T: Encodable + ?Sized>(value: &T) -> usize {
    value.length()
}
