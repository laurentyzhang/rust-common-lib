//! Concurrent storage slots, scoped to the owning EVM account.
//!
//! ARCO(4) | version(1) | API(1) | reserved(1) | field(1) | key(24)
//! Slot layout in big-endian byte order:
//! ```text
//! Bytes 0..4   : "ARCO" namespace marker
//! Byte  4      : format version (defined in version.rs)
//! Byte  5      : API address suffix, e.g. 0x84 or 0x85
//! Byte  6      : reserved (zero)
//! Byte  7      : field ID
//! Bytes 8..12  : zero padding
//! Bytes 12..32 : address handle (20 bytes)
//! ```
//!
//! The API byte matches [`ApiAddress`]'s call address suffix, not the storage owner.

use alloy_primitives::{Address, U256};

use super::{ApiAddress, version::VERSION};
use strum::IntoEnumIterator; // 👈 ADD THIS LINE AT THE TOP

const VERSION_OFFSET: usize = 4;
pub(super) const API_OFFSET: usize = 5;
const RESERVED_OFFSET: usize = 6;
pub(super) const FIELD_OFFSET: usize = 7;
const KEY_OFFSET: usize = 8;
const HANDLE_OFFSET: usize = 12;

/// Encodes and decodes concurrent storage slots.
/// ARCO(4) | version(1) | API(1) | reserved(1) | field(1) | key(24)
pub struct SlotCodec;

impl SlotCodec {
    /// Encode an address handle, left-padded to the 24-byte logical key.
    ///
    /// Fields of the same instance share a handle and use different field IDs.
    /// The owning account remains the address in `(address, slot)`.
    pub fn encode(api: ApiAddress, field: u8, handle: Address) -> U256 {
        let mut bytes = [0_u8; 32];
        bytes[..VERSION_OFFSET].copy_from_slice(b"ARCO");
        bytes[VERSION_OFFSET] = VERSION;
        bytes[API_OFFSET] = api as u8;
        bytes[FIELD_OFFSET] = field;
        bytes[HANDLE_OFFSET..].copy_from_slice(handle.as_slice());
        U256::from_be_bytes(bytes)
    }

    /// Selects a field while preserving the prefix and instance key.
    pub(super) fn with_field(slot: U256, field: u8) -> U256 {
        let mut bytes = slot.to_be_bytes::<32>();
        bytes[FIELD_OFFSET] = field;
        U256::from_be_bytes(bytes)
    }

    /// Checks whether a slot has a recognized API prefix and supported format version.
    pub fn is_api_slot(slot: U256) -> bool {
        let bytes = slot.to_be_bytes::<32>();
        return &bytes[..VERSION_OFFSET] == b"ARCO"
            && bytes[VERSION_OFFSET] == VERSION
            && ApiAddress::contains(bytes[API_OFFSET])
            && bytes[RESERVED_OFFSET] == 0;
    }

    /// Decode a recognized concurrent slot; unrelated or unknown prefixes return `None`.
    ///
    /// Returns `(api, field, key)`. Field IDs are interpreted by each API.
    pub fn decode(slot: U256) -> Option<(ApiAddress, u8, [u8; 24])> {
        if !Self::is_api_slot(slot) {
            return None;
        }

        // Require the full namespace and version before classifying.
        let bytes = slot.to_be_bytes::<32>();
        let api = ApiAddress::iter().find(|v| *v as u8 == bytes[API_OFFSET])?;
        let mut key = [0_u8; 24];
        key.copy_from_slice(&bytes[KEY_OFFSET..]);
        Some((api, bytes[FIELD_OFFSET], key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn is_api_slot_every_api_and_field() {
        for api in ApiAddress::iter() {
            for handle in [Address::ZERO, Address::repeat_byte(0xff)] {
                for field in u8::MIN..=u8::MAX {
                    let slot = SlotCodec::encode(api, field, handle);
                    assert!(SlotCodec::is_api_slot(slot));
                    assert_eq!(SlotCodec::decode(slot).unwrap().0, api);
                }
            }
        }
    }

    #[test]
    fn api_address_suffixes_are_not_api_slot_keys() {
        for api in ApiAddress::iter() {
            for slot in [U256::ZERO, U256::from(api as u8), U256::MAX] {
                assert!(!SlotCodec::is_api_slot(slot));
            }
        }
    }

    #[test]
    fn foreign_namespaces_are_rejected_for_every_api() {
        for api in ApiAddress::iter() {
            let mut bytes = SlotCodec::encode(api, 0, Address::ZERO).to_be_bytes::<32>();
            bytes[0] ^= 0xff;
            assert!(!SlotCodec::is_api_slot(U256::from_be_bytes(bytes)));
        }
    }

    #[test]
    fn round_trip_all_api_namespaces_and_fields() {
        let mut slots = HashSet::new();
        for api in ApiAddress::iter() {
            for handle in [
                Address::ZERO,
                Address::repeat_byte(0xff),
                Address::from(std::array::from_fn::<_, 20, _>(|i| i as u8)),
            ] {
                let mut key = [0; 24];
                key[HANDLE_OFFSET - KEY_OFFSET..].copy_from_slice(handle.as_slice());
                for field in [0, 1, 2, 3, 255] {
                    let slot = SlotCodec::encode(api, field, handle);
                    assert_eq!(SlotCodec::decode(slot), Some((api, field, key)));
                    assert!(slots.insert(slot));
                }
            }
        }
    }

    #[test]
    fn prefix_contains_api_address_and_field() {
        let slot = SlotCodec::encode(ApiAddress::Container, 3, Address::repeat_byte(0xab));
        let bytes = slot.to_be_bytes::<32>();
        assert_eq!(
            &bytes[..KEY_OFFSET],
            &[b'A', b'R', b'C', b'O', VERSION, 0x84, 0, 3]
        );
        assert_eq!(&bytes[KEY_OFFSET..HANDLE_OFFSET], &[0; 4]);
        assert_eq!(&bytes[HANDLE_OFFSET..], &[0xab; 20]);
    }

    #[test]
    fn handle_is_left_padded_without_changing_the_prefix() {
        for handle in [
            Address::ZERO,
            Address::repeat_byte(0xff),
            Address::with_last_byte(1),
        ] {
            let slot = SlotCodec::encode(ApiAddress::Scalar, 2, handle);
            let bytes = slot.to_be_bytes::<32>();
            assert_eq!(
                &bytes[..KEY_OFFSET],
                &[b'A', b'R', b'C', b'O', VERSION, 0x85, 0, 2]
            );
            assert_eq!(&bytes[KEY_OFFSET..HANDLE_OFFSET], &[0; 4]);
            assert_eq!(&bytes[HANDLE_OFFSET..], handle.as_slice());
        }
    }

    #[test]
    fn rejects_native_slots_and_unknown_prefix_fields() {
        assert_eq!(SlotCodec::decode(U256::ZERO), None);
        assert_eq!(SlotCodec::decode(U256::MAX), None);

        let slot = SlotCodec::encode(ApiAddress::Scalar, 0, Address::ZERO);
        for (index, value) in [
            (0, 0),
            (1, 0),
            (2, 0),
            (3, 0),
            (VERSION_OFFSET, VERSION.wrapping_add(1)),
            (API_OFFSET, 0xff),
            (RESERVED_OFFSET, 0xff),
        ] {
            let mut bytes = slot.to_be_bytes::<32>();
            bytes[index] = value;
            assert_eq!(SlotCodec::decode(U256::from_be_bytes(bytes)), None);
            assert!(!SlotCodec::is_api_slot(U256::from_be_bytes(bytes)));
        }
    }

    #[test]
    fn only_the_current_version_is_recognized() {
        let slot = SlotCodec::encode(ApiAddress::Scalar, 0, Address::ZERO);
        for version in u8::MIN..=u8::MAX {
            let mut bytes = slot.to_be_bytes::<32>();
            bytes[VERSION_OFFSET] = version;
            assert_eq!(
                SlotCodec::decode(U256::from_be_bytes(bytes)).is_some(),
                version == VERSION
            );
            assert_eq!(
                SlotCodec::is_api_slot(U256::from_be_bytes(bytes)),
                version == VERSION
            );
        }
    }
}
