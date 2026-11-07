use strum_macros::{EnumIter, FromRepr};

use crate::namespace::{Namespace, NamespaceCodec};

use alloy_primitives::{Address, U256};

/// Arcology's reserved EVM call targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, EnumIter, FromRepr)]
#[repr(u8)]
pub enum ApiAddress {
    Set = Namespace::Set as u8,
    U256 = Namespace::U256 as u8,
    I256 = Namespace::I256 as u8,
    Runtime = Namespace::Runtime as u8,
    Multiprocess = Namespace::Multiprocess as u8,
}

impl ApiAddress {
    pub fn address(self) -> Address {
        Self::arcology_address(self as u8)
    }

    pub fn contains(address: u8) -> bool {
        Self::from_repr(address).is_some()
    }

    pub fn is_storage_key(key: U256) -> bool {
        NamespaceCodec::decode(&key.to_be_bytes::<32>())
            .is_some_and(|(namespace, _)| Self::try_from(namespace).is_ok())
    }

    /// Builds the canonical low Arcology address for a one-byte suffix.
    fn arcology_address(suffix: u8) -> Address {
        let mut bytes = [0_u8; 20];
        bytes[19] = suffix;
        Address::from(bytes)
    }
}

impl From<ApiAddress> for Namespace {
    fn from(api: ApiAddress) -> Self {
        Self::from_repr(api as u8).expect("API IDs are registered namespaces")
    }
}

impl TryFrom<Namespace> for ApiAddress {
    type Error = ();

    fn try_from(namespace: Namespace) -> Result<Self, Self::Error> {
        Self::from_repr(namespace as u8).ok_or(())
    }
}
