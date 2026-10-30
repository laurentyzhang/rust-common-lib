//! Slot payload: field(1) | key(24), preceded by a namespace-codec prefix.

use alloy_primitives::{Address, U256};

use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Encodes and decodes slot fields and instance keys.
pub struct KeyCodec;

impl KeyCodec {
    /// Encodes an address handle, left-padded to the 24-byte key.
    pub fn encode(namespace: impl Into<Namespace>, field: u8, handle: Address) -> U256 {
        let mut key = [0; 24];
        key[4..].copy_from_slice(handle.as_slice());
        Self::encode_key(namespace.into(), field, key)
    }

    pub fn encode_key(namespace: Namespace, field: u8, key: [u8; 24]) -> U256 {
        let prefix = NamespaceCodec::encode(namespace);
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&prefix);
        bytes[PREFIX_LEN] = field;
        bytes[PREFIX_LEN + 1..].copy_from_slice(&key);
        U256::from_be_bytes(bytes)
    }

    /// Selects a field while preserving the prefix and instance key.
    pub(in crate::eu) fn with_field(key: U256, field: u8) -> U256 {
        let mut bytes = key.to_be_bytes::<32>();
        bytes[PREFIX_LEN] = field;
        U256::from_be_bytes(bytes)
    }

    pub fn decode(key: U256) -> Option<(Namespace, u8, [u8; 24])> {
        let bytes = key.to_be_bytes::<32>();
        let (namespace, payload) = NamespaceCodec::decode(&bytes)?;
        let (&field, key) = payload.split_first()?;
        Some((namespace, field, key.try_into().ok()?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eu::ApiAddress;
    use strum::IntoEnumIterator;

    #[test]
    fn round_trip_slot_fields_and_keys() {
        for namespace in Namespace::iter() {
            for field in u8::MIN..=u8::MAX {
                for key in [[0; 24], [0xff; 24]] {
                    let slot = KeyCodec::encode_key(namespace, field, key);
                    assert_eq!(KeyCodec::decode(slot), Some((namespace, field, key)));
                    let bytes = slot.to_be_bytes::<32>();
                    assert_eq!(&bytes[..PREFIX_LEN], &NamespaceCodec::encode(namespace));
                }
            }
        }
    }

    #[test]
    fn address_handles_are_left_padded() {
        for api in ApiAddress::iter() {
            for handle in [Address::ZERO, Address::repeat_byte(0xff)] {
                let slot = KeyCodec::encode(api, 2, handle);
                let (namespace, field, key) = KeyCodec::decode(slot).unwrap();
                assert_eq!(namespace, api.into());
                assert_eq!(field, 2);
                assert_eq!(&key[..4], &[0; 4]);
                assert_eq!(&key[4..], handle.as_slice());
                assert!(ApiAddress::is_storage_key(slot));
            }
        }
    }

    #[test]
    fn changing_field_preserves_prefix_and_key() {
        let slot = KeyCodec::encode_key(Namespace::Conflict, 0, [42; 24]);
        let changed = KeyCodec::with_field(slot, 1);
        assert_eq!(
            KeyCodec::decode(changed),
            Some((Namespace::Conflict, 1, [42; 24]))
        );
        assert_eq!(
            &slot.to_be_bytes::<32>()[..PREFIX_LEN],
            &changed.to_be_bytes::<32>()[..PREFIX_LEN]
        );
    }

    #[test]
    fn decode_requires_a_valid_namespace_prefix() {
        for slot in [U256::ZERO, U256::MAX] {
            assert_eq!(KeyCodec::decode(slot), None);
            assert!(!ApiAddress::is_storage_key(slot));
        }
        let slot = KeyCodec::encode_key(Namespace::Conflict, 1, [7; 24]);
        for index in 0..PREFIX_LEN {
            let mut bytes = slot.to_be_bytes::<32>();
            bytes[index] ^= 0xff;
            assert_eq!(KeyCodec::decode(U256::from_be_bytes(bytes)), None);
        }
    }
}
