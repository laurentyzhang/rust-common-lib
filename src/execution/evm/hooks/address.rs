use alloy_primitives::Address;

/// Arcology's reserved EVM call targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ApiAddress {
    Io = 0x60,
    Container = 0x84,
    Scalar = 0x85,
    Runtime = 0xa0,
    Multiprocess = 0xb0,
}

impl ApiAddress {
    pub const ALL: [Self; 5] = [
        Self::Io,
        Self::Container,
        Self::Scalar,
        Self::Runtime,
        Self::Multiprocess,
    ];

    pub fn address(self) -> Address {
        arcology_address(self as u8)
    }
}

/// Builds the canonical low Arcology address for a one-byte suffix.
pub fn arcology_address(suffix: u8) -> Address {
    let mut bytes = [0_u8; 20];
    bytes[19] = suffix;
    Address::from(bytes)
}
