use alloy_primitives::{Address, U256, keccak256};

use super::{Property, ScalarKind};
use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Layout: [namespace prefix:7][property:1][instance key:24].
pub struct ScalarKeyCodec;

impl ScalarKeyCodec {
    /// Uses a zero-padded address as the instance key.
    pub fn encode(namespace: Namespace, property: u8, handle: Address) -> U256 {
        let mut instance = [0; 24];
        instance[4..].copy_from_slice(handle.as_slice());
        Self::encode_key(namespace, property, instance)
    }

    /// Derives the internal scalar key for a caller-supplied key, keeping the format private.
    /// The instance ID is the first 24 bytes of the key's keccak hash, under `namespace`.
    pub fn derive_key(namespace: Namespace, user_key: U256) -> U256 {
        let instance = keccak256(user_key.to_be_bytes::<32>())[..24]
            .try_into()
            .unwrap();
        Self::encode_key(namespace, Property::Value as u8, instance)
    }

    pub fn encode_key(namespace: Namespace, property: u8, instance: [u8; 24]) -> U256 {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(namespace));
        bytes[PREFIX_LEN] = property;
        bytes[PREFIX_LEN + 1..].copy_from_slice(&instance);
        U256::from_be_bytes(bytes)
    }

    pub(in crate::execution) fn with_property(key: U256, property: u8) -> U256 {
        let mut bytes = key.to_be_bytes::<32>();
        bytes[PREFIX_LEN] = property;
        U256::from_be_bytes(bytes)
    }

    /// Rejects other namespaces and undefined scalar properties.
    pub fn decode(key: U256) -> Option<(ScalarKind, u8, [u8; 24])> {
        let (namespace, payload) = NamespaceCodec::decode(key)?;
        let kind = ScalarKind::try_from(namespace).ok()?;
        let (&property, instance) = payload.split_first()?;
        Property::from_repr(property)?;
        Some((kind, property, instance.try_into().ok()?))
    }
}
