use alloy_primitives::{U256, keccak256};

use crate::execution::Codec;
use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Set field IDs encoded in byte 13 of a slot key, after the set ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq, strum_macros::FromRepr)]
#[repr(u8)]
pub enum SetProperty {
    Length = 0,          // Committed element count.
    Tail = 1,            // Last committed element; the first is Next(Tail).
    Next = 2,            // Committed element after x; flag 1 marks "the next element is 0".
    Added = 3,           // Pending position + 1 of an added element; temporary until commit.
    Removed = 4,         // Pending removal flag of a committed element; temporary until commit.
    DeltaAdded = 5,      // Pending additions log, one element per word, zero when cancelled; temporary until commit.
    PendingRemovals = 6, // Pending removals of committed elements, ascending, one per word; temporary until commit.
    DeltaCancelled = 7,  // Bitmap of cancelled pending additions, 256 positions per word; temporary until commit.
}

const SET_ID_LEN: usize = 6;
const ID_END: usize = PREFIX_LEN + SET_ID_LEN + 1 + 16;

/// Layout: [namespace prefix:7][set ID:6][property:1][element hash or index:16][flag:1][reserved:1].
/// The owner is the slot's account; set IDs are private hashes of the caller's set key.
pub struct SetKeyCodec;

impl SetKeyCodec {
    /// Builds a Set slot key from its set ID, property, 16-byte ID and flag; the reserved byte is zero.
    pub fn encode(set: [u8; 6], property: SetProperty, id: [u8; 16], flag: u8) -> U256 {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(Namespace::Set));
        bytes[PREFIX_LEN..PREFIX_LEN + SET_ID_LEN].copy_from_slice(&set);
        bytes[PREFIX_LEN + SET_ID_LEN] = property as u8;
        bytes[PREFIX_LEN + SET_ID_LEN + 1..ID_END].copy_from_slice(&id);
        bytes[ID_END] = flag;
        U256::from_be_bytes(bytes)
    }

    /// Derives a set's private 6-byte ID from the caller's set key, keeping the format private.
    pub fn derive_key(set: U256) -> [u8; 6] {
        keccak256(set.to_be_bytes::<32>())[..SET_ID_LEN].try_into().unwrap()
    }

    /// Builds the slot key holding a set's committed element count.
    pub fn length(set: [u8; 6]) -> U256 {
        Self::encode(set, SetProperty::Length, [0; 16], 0)
    }

    /// Builds the slot key holding a set's last committed element.
    pub fn tail(set: [u8; 6]) -> U256 {
        Self::encode(set, SetProperty::Tail, [0; 16], 0)
    }

    /// Builds the slot key of a per-element property (Next, Added, Removed), keyed by the element's hash.
    pub fn element(property: SetProperty, set: [u8; 6], element: U256, flag: u8) -> U256 {
        let hash = keccak256(element.to_be_bytes::<32>());
        Self::encode(set, property, hash[..16].try_into().unwrap(), flag)
    }

    /// Returns the codec storing one pending log (DeltaAdded, PendingRemovals or DeltaCancelled):
    /// [namespace prefix:7][set ID:6][property:1][index:16][flag:1 = 0][reserved:1 = 0].
    pub fn delta(property: SetProperty, set: [u8; 6]) -> Codec {
        let mut prefix = NamespaceCodec::encode(Namespace::Set).to_vec();
        prefix.extend_from_slice(&set);
        prefix.push(property as u8);
        Codec::new(prefix, vec![0, 0])
    }

    /// Returns the set ID, property, 16-byte ID and flag of a key, rejecting other namespaces,
    /// undefined properties and a non-zero reserved byte.
    pub fn decode(key: U256) -> Option<([u8; 6], SetProperty, [u8; 16], u8)> {
        let (namespace, _) = NamespaceCodec::decode(key)?;
        let bytes = key.to_be_bytes::<32>();
        if namespace != Namespace::Set || bytes[31] != 0 {
            return None;
        }
        Some((
            bytes[PREFIX_LEN..PREFIX_LEN + SET_ID_LEN].try_into().ok()?,
            SetProperty::from_repr(bytes[PREFIX_LEN + SET_ID_LEN])?,
            bytes[PREFIX_LEN + SET_ID_LEN + 1..ID_END].try_into().ok()?,
            bytes[ID_END],
        ))
    }
}
