use alloy_primitives::{Address, U256};

use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Contract metadata fields. Counts measure peers, with four peers per index slot.
#[derive(Clone, Copy, strum_macros::FromRepr)]
#[repr(u8)]
pub enum MetaFieldId {
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
impl MetaFieldId {
    /// Selects this property for the host function and packed slot index.
    pub fn slot(self, host: Address, callee: u64, index: u32) -> U256 {
        MetaKeyCodec::encode(self as u8, host, callee, index)
    }
}

/// Layout: [namespace prefix:7][selector:4][property:1][slot index:4][host prefix:16].
pub struct MetaKeyCodec;

impl MetaKeyCodec {
    /// Recognizes metadata fields with CRDT updates.
    pub fn is_crdt(key: U256) -> bool {
        matches!(Self::decode(key), Some((property, _, _, _))         if property == MetaFieldId::DeferredGas as u8)
    }

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
        let (namespace, payload) = NamespaceCodec::decode(key)?;
        if namespace != Namespace::Meta {
            return None;
        }
        let property = payload[4];
        MetaFieldId::from_repr(property)?;
        Some((
            property,
            payload[..4].try_into().ok()?,
            u32::from_be_bytes(payload[5..9].try_into().ok()?),
            payload[9..].try_into().ok()?,
        ))
    }
}
