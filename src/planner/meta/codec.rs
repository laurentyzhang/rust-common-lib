use alloy_primitives::{Address, U256};

use super::property::Property;
use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Layout: [namespace prefix:7][selector:4][property:1][slot index:4][host prefix:16].
pub struct MetaKeyCodec;

impl MetaKeyCodec {
    pub fn encode(property: u8, host: Address, callee: u64, index: u32) -> U256 {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(Namespace::Meta));
        bytes[7..11].copy_from_slice(&callee.to_le_bytes()[4..]);
        bytes[11] = property;
        bytes[12..16].copy_from_slice(&index.to_be_bytes());
        bytes[16..].copy_from_slice(&host[..16]);
        U256::from_be_bytes(bytes)
    }

    /// Returns property, selector, slot index, and host address prefix.
    pub fn decode(key: U256) -> Option<(u8, [u8; 4], u32, [u8; 16])> {
        let bytes = key.to_be_bytes::<32>();
        let (namespace, payload) = NamespaceCodec::decode(&bytes)?;
        if namespace != Namespace::Meta {
            return None;
        }
        let property = payload[4];
        Property::from_repr(property)?;
        Some((
            property,
            payload[..4].try_into().ok()?,
            u32::from_be_bytes(payload[5..9].try_into().ok()?),
            payload[9..].try_into().ok()?,
        ))
    }
}
