//! Typed concurrent storage slots, scoped to the owning EVM account.
//!
//! Slot layout in big-endian byte order:
//! ```text
//! Bytes 0..4   : "ARCO" namespace marker
//! Byte  4      : format version (1)
//! Byte  5      : API address suffix, e.g. 0x84 or 0x85
//! Byte  6      : storage/merge type tag
//! Byte  7      : reserved (0)
//! Bytes 8..32  : logical key (24 bytes)
//! ```
//!
//! The API byte matches [`ApiAddress`]'s call address suffix, not the storage owner.
//! These type tags describe storage semantics; they are not Solidity ABI type IDs.

use alloy_primitives::U256;

use super::ApiAddress;

/// How a concurrent value should be interpreted by the state merger.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Type {
    /// Ordinary data, including byte payload words, bounds, and lifecycle metadata.
    NonCommutative = 1,
    /// The current value of an unsigned cumulative scalar, excluding its bounds.
    CumulativeU256 = 2,
    /// The current value of a signed cumulative scalar, excluding its bounds.
    CumulativeI256 = 3,
    /// Container membership/set records; distinct from payload and index metadata.
    Set = 4,
}

/// Adds and recognizes the eight-byte concurrent storage prefix.
///
/// Pass the encoded slot to `EvmInternals::sload` or `EvmInternals::sstore`,
/// alongside the owning account address. This type only constructs and classifies
/// slot keys; it does not access state or perform CRDT merge operations.
///
/// API namespaces and storage types are independent: container `0x84`, for example,
/// supports both ordinary payloads and cumulative values. Runtime `0xa0` and
/// multiprocess `0xb0` can use the same storage types without special merge rules
/// inferred solely from their API addresses.
pub struct SlotPrefixer;

impl SlotPrefixer {
    /// Encode an already-derived 24-byte logical key without truncating it.
    ///
    /// Callers must derive distinct keys for separate fields and payload words.
    /// The owning account remains the address in `(address, slot)`.
    /// For example, a container's cumulative value uses `Container` with
    /// `CumulativeU256`, while its bounds use `Container` with `NonCommutative`.
    pub fn encode(api: ApiAddress, kind: Type, key: [u8; 24]) -> U256 {
        let mut bytes = [0_u8; 32];
        // Build the eight-byte prefix; the final byte is reserved for future use.
        bytes[..8].copy_from_slice(&[b'A', b'R', b'C', b'O', 1, api as u8, kind as u8, 0]);
        bytes[8..].copy_from_slice(&key);
        U256::from_be_bytes(bytes)
    }

    /// Decode a recognized concurrent slot; unrelated or unknown prefixes return `None`.
    ///
    /// Returns `(api, kind, key)`: the API namespace, storage/merge type, and the
    /// original 24-byte logical key. All prefix fields must match this format.
    pub fn decode(slot: U256) -> Option<(ApiAddress, Type, [u8; 24])> {
        let bytes = slot.to_be_bytes::<32>();
        // Require the full namespace, version, and reserved byte before classifying.
        if &bytes[..5] != b"ARCO\x01" || bytes[7] != 0 {
            return None;
        }

        let api = ApiAddress::ALL
            .into_iter()
            .find(|api| *api as u8 == bytes[5])?;
        // The type selects storage semantics independently of the API address.
        let kind = match bytes[6] {
            1 => Type::NonCommutative,
            2 => Type::CumulativeU256,
            3 => Type::CumulativeI256,
            4 => Type::Set,
            _ => return None,
        };
        let mut key = [0_u8; 24];
        key.copy_from_slice(&bytes[8..]);
        Some((api, kind, key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn round_trip_all_api_namespaces_and_storage_types() {
        let kinds = [
            Type::NonCommutative,
            Type::CumulativeU256,
            Type::CumulativeI256,
            Type::Set,
        ];
        let mut slots = HashSet::new();
        for api in ApiAddress::ALL {
            for kind in kinds {
                for key in [[0; 24], [0xff; 24], std::array::from_fn(|i| i as u8)] {
                    let slot = SlotPrefixer::encode(api, kind, key);
                    assert_eq!(SlotPrefixer::decode(slot), Some((api, kind, key)));
                    assert!(slots.insert(slot));
                }
            }
        }
    }

    #[test]
    fn prefix_contains_api_address_and_storage_type() {
        let slot = SlotPrefixer::encode(ApiAddress::Container, Type::CumulativeU256, [0xab; 24]);
        let bytes = slot.to_be_bytes::<32>();
        assert_eq!(&bytes[..8], &[b'A', b'R', b'C', b'O', 1, 0x84, 2, 0]);
        assert_eq!(&bytes[8..], &[0xab; 24]);
    }

    #[test]
    fn rejects_native_slots_and_unknown_prefix_fields() {
        assert_eq!(SlotPrefixer::decode(U256::ZERO), None);
        assert_eq!(SlotPrefixer::decode(U256::MAX), None);

        let slot = SlotPrefixer::encode(ApiAddress::Scalar, Type::CumulativeU256, [0; 24]);
        for (index, value) in [
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (4, 2),
            (5, 0xff),
            (6, 0xff),
            (7, 1),
        ] {
            let mut bytes = slot.to_be_bytes::<32>();
            bytes[index] = value;
            assert_eq!(SlotPrefixer::decode(U256::from_be_bytes(bytes)), None);
        }
    }
}
