use alloy_primitives::Address;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CalleeId {
    pub account: Address,
    pub selector: [u8; 4],
    pub compact: u64,
}

impl CalleeId {
    /// Creates a callee ID and computes its compact key.
    pub fn new(account: Address, selector: [u8; 4]) -> Self {
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&account[..4]);
        bytes[4..].copy_from_slice(&selector);
        Self {
            account,
            selector,
            compact: u64::from_le_bytes(bytes),
        }
    }
}
