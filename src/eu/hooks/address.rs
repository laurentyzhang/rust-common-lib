use strum::IntoEnumIterator;
use strum_macros::EnumIter;

use alloy_primitives::Address;

/// Arcology's reserved EVM call targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, EnumIter)]
#[repr(u8)]
pub enum ApiAddress {
    Io = 0x60,
    Container = 0x84,
    Scalar = 0x85,
    Runtime = 0xa0,
    Multiprocess = 0xb0,
}

impl ApiAddress {
    pub fn address(self) -> Address {
        Self::arcology_address(self as u8)
    }

    pub fn contains(address: u8) -> bool {
        Self::iter().any(|variant| variant as u8 == address)
    }

    /// Builds the canonical low Arcology address for a one-byte suffix.
    fn arcology_address(suffix: u8) -> Address {
        let mut bytes = [0_u8; 20];
        bytes[19] = suffix;
        Address::from(bytes)
    }
}
