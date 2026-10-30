use alloy_primitives::U256;

use crate::{eu::KeyCodec, namespace::Namespace};

/// Conflict-history fields stored under the owning contract.
/// Count values are U256; entries contain zero-padded 20-byte addresses.
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Field {
    Count = 0,
    Entry = 1,
}

impl Field {
    /// Encodes an entry index as a big-endian, zero-padded 24-byte key.
    /// The count record uses index zero.
    pub fn slot(self, index: u64) -> U256 {
        let mut key = [0; 24];
        key[16..].copy_from_slice(&index.to_be_bytes());
        KeyCodec::encode_key(Namespace::Conflict, self as u8, key)
    }
}
