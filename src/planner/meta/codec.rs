use alloy_evm::block::BlockExecutionError;
use alloy_primitives::{Address, U256};

use crate::common::codec::CodecError;
use crate::execution::Codec;
use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Contract metadata fields. Peer lists are stored through `MetaFieldId::codec`.
#[derive(Clone, Copy, strum_macros::FromRepr)]
#[repr(u8)]
pub enum MetaFieldId {
    /// Reserved for future metadata.
    Reserved = 0,
    /// U256::MAX marks the function sequential-only.
    ConflictCount = 1,
    /// Conflict peer list: index 0 holds its byte length, indexes 1.. its chunks.
    ConflictIndex = 2,
    /// Deferred peer list: index 0 holds its byte length, indexes 1.. its chunks.
    DeferredIndex = 4,
    /// Required prepaid gas for this function; zero disables deferral.
    DeferredGas = 5,
}

// Key: [ARCO:4][version:1][namespace:1][reserved:1][selector:4][property:1][slot index:4][host prefix:16].
impl MetaFieldId {
    /// Selects this metadata field for the host function and slot index.
    pub fn slot(self, contract: Address, selector: [u8; 4], index: u32) -> U256 {
        MetaCodec::encode(self as u8, contract, selector, index)
    }

    /// Returns the slot codec for this field's list; its keys match `slot` for the same index.
    pub fn codec(self, contract: Address, selector: [u8; 4]) -> Codec {
        let mut prefix = NamespaceCodec::encode(Namespace::Meta).to_vec();
        prefix.extend_from_slice(&selector);
        prefix.push(self as u8);
        Codec::new(prefix, contract[..16].to_vec())
    }
}

impl From<CodecError> for BlockExecutionError {
    /// Reports a meta list codec failure as a block execution error.
    fn from(error: CodecError) -> Self {
        BlockExecutionError::other(error)
    }
}

/// Layout: [namespace prefix:7][selector:4][property:1][slot index:4][host prefix:16].
pub struct MetaCodec;

impl MetaCodec {
    /// Recognizes metadata fields with CRDT updates.
    pub fn is_crdt(key: U256) -> bool {
        matches!(Self::decode(key), Some((property, _, _, _)) if property == MetaFieldId::DeferredGas as u8)
    }

    pub fn encode(property: u8, contract: Address, selector: [u8; 4], index: u32) -> U256 {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(Namespace::Meta));
        bytes[7..11].copy_from_slice(&selector);
        bytes[11] = property;
        bytes[12..16].copy_from_slice(&index.to_be_bytes());
        bytes[16..].copy_from_slice(&contract[..16]);
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

    pub(crate) fn encode_u64(value: u64) -> U256 {
        U256::from(value)
    }

    pub(crate) fn decode_u64(value: U256) -> Option<u64> {
        value.try_into().ok()
    }
}
