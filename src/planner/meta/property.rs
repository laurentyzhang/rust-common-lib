use alloy_primitives::{Address, U256};

use super::MetaKeyCodec;

/// Contract metadata fields. Counts measure peers, with four peers per index slot.
#[derive(Clone, Copy, strum_macros::FromRepr)]
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
    /// Required prepaid gas for this function; zero disables deferral.
    DeferredGas = 5,
}

// Key: [ARCO:4][version:1][namespace:1][reserved:1][selector:4][property:1][slot index:4][host prefix:16].
// Example
// slot index 0 → peers 0–3
// slot index 1 → peers 4–7
// slot index 2 → peers 8–11
impl Property {
    /// Selects this property for the host function and packed slot index.
    pub fn slot(self, host: Address, callee: u64, index: u32) -> U256 {
        MetaKeyCodec::encode(self as u8, host, callee, index)
    }
}
