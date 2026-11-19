use alloy_primitives::Address;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CalleeId {
    pub account: Address,
    pub selector: [u8; 4],
    pub series: u64,
}

impl CalleeId {
    pub fn new(account: Address, selector: [u8; 4]) -> Self {
        Self {
            account,
            selector,
            series: 0,
        }
    }

    pub(crate) fn compact(self) -> u64 {
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&self.account[..4]);
        bytes[4..].copy_from_slice(&self.selector);
        u64::from_le_bytes(bytes)
    }
}
