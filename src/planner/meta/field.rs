use alloy_primitives::{Address, U256};

use crate::{eu::SlotKeyCodec, namespace::Namespace};

/// Contract metadata fields. Counts measure peers, with four peers per index slot.
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Property {
    /// Reserved for future metadata.
    Reserved = 0,
    /// Conflict peer count; U256::MAX means sequential-only.
    ConflictCount = 1,
    /// Up to four packed conflict peers at this slot index.
    ConflictIndex = 2,
    /// Deferred peer count for the host function.
    DeferredCount = 3,
    /// Up to four packed deferred peers at this slot index.
    DeferredIndex = 4,
}

// namespace; API address
// Key: [ARCO:4][version:1][namespace:1][reserved:1][field:1][selector:4][slot index:4][host address prefix:16].
// Example
// slot index 0 → peers 0–3
// slot index 1 → peers 4–7
// slot index 2 → peers 8–11
impl Property {
    /// Payload: host selector (4), big-endian slot index (4), host address prefix (16).
    pub fn slot(self, host: Address, callee: u64, index: u32) -> U256 {
        let mut key = [0; 24];
        key[..4].copy_from_slice(&callee.to_le_bytes()[4..]);
        key[4..8].copy_from_slice(&index.to_be_bytes());
        key[8..].copy_from_slice(&host[..16]);
        SlotKeyCodec::encode_key(Namespace::Meta, self as u8, key)
    }
}
