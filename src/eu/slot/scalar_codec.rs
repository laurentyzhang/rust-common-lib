use alloy_primitives::{Address, U256};

use super::scalar::Property;
use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Layout: [namespace prefix:7][property:1][instance key:24].
pub struct ScalarKeyCodec;

impl ScalarKeyCodec {
    /// Uses a zero-padded address as the instance key.
    pub fn encode(property: u8, handle: Address) -> U256 {
        let mut instance = [0; 24];
        instance[4..].copy_from_slice(handle.as_slice());
        Self::encode_key(property, instance)
    }

    pub fn encode_key(property: u8, instance: [u8; 24]) -> U256 {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(Namespace::Scalar));
        bytes[PREFIX_LEN] = property;
        bytes[PREFIX_LEN + 1..].copy_from_slice(&instance);
        U256::from_be_bytes(bytes)
    }

    pub(in crate::eu) fn with_property(key: U256, property: u8) -> U256 {
        let mut bytes = key.to_be_bytes::<32>();
        bytes[PREFIX_LEN] = property;
        U256::from_be_bytes(bytes)
    }

    /// Rejects other namespaces and undefined scalar properties.
    pub fn decode(key: U256) -> Option<(u8, [u8; 24])> {
        let bytes = key.to_be_bytes::<32>();
        let (namespace, payload) = NamespaceCodec::decode(&bytes)?;
        if namespace != Namespace::Scalar {
            return None;
        }
        let (&property, instance) = payload.split_first()?;
        Property::from_repr(property)?;
        Some((property, instance.try_into().ok()?))
    }
}
