use alloy_primitives::{U256, keccak256};

use crate::execution::Codec;
use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Set field IDs encoded in byte 13 of a slot key, after the set ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq, strum_macros::FromRepr)]
#[repr(u8)]
pub enum Property {
    Length = 0,          // Live element count, in both formats.
    Head = 1,            // Compact format: first element of the list.
    Tail = 2,            // Compact format: last element of the list.
    Node = 3,            // Compact format: element after x; empty for Tail, and an empty Node before Tail means 0.
    Added = 4,           // Pending: position + 1 of an added element in DeltaAdded; dropped at generation commit.
    Removed = 5,         // Pending: removal flag of an indexed element; dropped at generation commit.
    DeltaAdded = 6,      // Pending: additions log, one element per word, zero when cancelled; dropped at generation commit.
    PendingRemovals = 7, // Pending: removals of indexed elements, ascending, one per word; dropped at generation commit.
    DeltaCancelled = 8,  // Pending: bitmap of cancelled additions, 256 positions per word; dropped at generation commit.
    Index = 9,           // Indexed format: elements in order, one per word, zero when removed; cleared by compact.
    Position = 10,       // Indexed format: position + 1 of an element in Index; cleared by compact.
    Holes = 11,          // Indexed format: bitmap of removed Index positions, 256 per word; cleared by compact.
    Indexed = 12,        // Indexed format: marks the set as expanded; cleared by compact.
}

const SET_ID_LEN: usize = 6;
const ID_END: usize = PREFIX_LEN + SET_ID_LEN + 1 + 16;

/// Layout: [namespace prefix:7][set ID:6][property:1][element hash or index:16][reserved:2].
/// The owner is the slot's account; set IDs are private hashes of the caller's set key.
pub struct KeyCodec;

impl KeyCodec {
    /// Builds a Set slot key from its set ID, property and 16-byte ID; the reserved bytes are zero.
    pub fn encode(set: [u8; 6], property: Property, id: [u8; 16]) -> U256 {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(Namespace::Set));
        bytes[PREFIX_LEN..PREFIX_LEN + SET_ID_LEN].copy_from_slice(&set);
        bytes[PREFIX_LEN + SET_ID_LEN] = property as u8;
        bytes[PREFIX_LEN + SET_ID_LEN + 1..ID_END].copy_from_slice(&id);
        U256::from_be_bytes(bytes)
    }

    /// Derives a set's private 6-byte ID from the caller's set key, keeping the format private.
    pub fn derive_key(set: U256) -> [u8; 6] {
        keccak256(set.to_be_bytes::<32>())[..SET_ID_LEN].try_into().unwrap()
    }

    /// Builds the slot key holding a set's live element count.
    pub fn length(set: [u8; 6]) -> U256 {
        Self::encode(set, Property::Length, [0; 16])
    }

    /// Builds the slot key holding the first element of a set's list (compact format).
    pub fn head(set: [u8; 6]) -> U256 {
        Self::encode(set, Property::Head, [0; 16])
    }

    /// Builds the slot key holding the last element of a set's list (compact format).
    pub fn tail(set: [u8; 6]) -> U256 {
        Self::encode(set, Property::Tail, [0; 16])
    }

    /// Builds the slot key marking a set as expanded into the indexed format.
    pub fn indexed(set: [u8; 6]) -> U256 {
        Self::encode(set, Property::Indexed, [0; 16])
    }

    /// Builds the slot key of a per-element property (Node, Added, Removed, Position), keyed by the element's hash.
    pub fn element(property: Property, set: [u8; 6], element: U256) -> U256 {
        let hash = keccak256(element.to_be_bytes::<32>());
        Self::encode(set, property, hash[..16].try_into().unwrap())
    }

    /// Returns the codec storing one log (DeltaAdded, PendingRemovals, DeltaCancelled, Index or Holes):
    /// [namespace prefix:7][set ID:6][property:1][index:16][reserved:2 = 0].
    pub fn delta(property: Property, set: [u8; 6]) -> Codec {
        let mut prefix = NamespaceCodec::encode(Namespace::Set).to_vec();
        prefix.extend_from_slice(&set);
        prefix.push(property as u8);
        Codec::new(prefix, vec![0, 0])
    }

    /// Returns the set ID, property and 16-byte ID of a key, rejecting other namespaces,
    /// undefined properties and non-zero reserved bytes.
    pub fn decode(key: U256) -> Option<([u8; 6], Property, [u8; 16])> {
        let (namespace, _) = NamespaceCodec::decode(key)?;
        let bytes = key.to_be_bytes::<32>();
        if namespace != Namespace::Set || bytes[ID_END..] != [0, 0] {
            return None;
        }
        Some((
            bytes[PREFIX_LEN..PREFIX_LEN + SET_ID_LEN].try_into().ok()?,
            Property::from_repr(bytes[PREFIX_LEN + SET_ID_LEN])?,
            bytes[PREFIX_LEN + SET_ID_LEN + 1..ID_END].try_into().ok()?,
        ))
    }
}
