use alloy_primitives::{U256, keccak256};

use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Set field IDs encoded in byte 7 of a slot key.
#[derive(Clone, Copy, Debug, Eq, PartialEq, strum_macros::FromRepr)]
#[repr(u8)]
pub enum SetProperty {
    Length = 0,        // Number of allocated index slots, including holes.
    Index = 1,         // Element ID at an index; zero marks a removed slot.
    Member = 2,        // Committed index + 1 of an element; zero when absent.
    Added = 3,         // Pending insert flag.
    Removed = 4,       // Pending remove flag.
    Size = 5,          // Element byte length.
    Chunk = 6,         // One 32-byte chunk of element data.
    PendingLength = 7, // Number of entries in the pending list.
    Pending = 8,       // Element ID flagged since the last fold, in first-flag order.
}

/// Layout: [namespace prefix:7][property:1][id:24]. The owner is the slot's account.
pub struct SetKeyCodec;

impl SetKeyCodec {
    /// Builds a Set slot key from a property and a 24-byte ID.
    pub fn encode(property: SetProperty, id: [u8; 24]) -> U256 {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(Namespace::Set));
        bytes[PREFIX_LEN] = property as u8;
        bytes[PREFIX_LEN + 1..].copy_from_slice(&id);
        U256::from_be_bytes(bytes)
    }

    /// Derives an element's ID from the first 24 bytes of its keccak hash.
    pub fn element_id(element: &[u8]) -> [u8; 24] {
        keccak256(element)[..24].try_into().unwrap()
    }

    /// Builds the slot key holding the element ID at index `i`.
    pub fn index(i: u64) -> U256 {
        let mut id = [0; 24];
        id[16..].copy_from_slice(&i.to_be_bytes());
        Self::encode(SetProperty::Index, id)
    }

    /// Builds the slot key holding the element ID at pending-list position `k`.
    pub fn pending(k: u64) -> U256 {
        let mut id = [0; 24];
        id[16..].copy_from_slice(&k.to_be_bytes());
        Self::encode(SetProperty::Pending, id)
    }

    /// Builds the slot key holding chunk `j` of an element's data.
    pub fn chunk(id: [u8; 24], j: u64) -> U256 {
        let mut input = [0; 32];
        input[..24].copy_from_slice(&id);
        input[24..].copy_from_slice(&j.to_be_bytes());
        Self::encode(
            SetProperty::Chunk,
            keccak256(input)[..24].try_into().unwrap(),
        )
    }

    /// Returns the property and ID of a key, rejecting other namespaces and undefined properties.
    pub fn decode(key: U256) -> Option<(SetProperty, [u8; 24])> {
        let (namespace, payload) = NamespaceCodec::decode(key)?;
        if namespace != Namespace::Set {
            return None;
        }
        let (&property, id) = payload.split_first()?;
        Some((SetProperty::from_repr(property)?, id.try_into().ok()?))
    }
}
