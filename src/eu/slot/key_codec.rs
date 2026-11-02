//! Slot payload: meta(1) | key(24), preceded by a namespace-codec prefix.

use alloy_primitives::{Address, U256};

use crate::namespace::{Namespace, NamespaceCodec, codec::PREFIX_LEN};

/// Encodes and decodes slot fields and instance keys.
/// Layout: [ARCO:4][version:1][namespace:1][reserved:1][meta:1][instance key:24].
pub struct SlotKeyCodec;

impl SlotKeyCodec {
    /// Encodes an address handle, left-padded to the 24-byte key.
    pub fn encode(namespace: impl Into<Namespace>, meta: u8, handle: Address) -> U256 {
        let mut key = [0; 24];
        key[4..].copy_from_slice(handle.as_slice());
        Self::encode_key(namespace.into(), meta, key)
    }

    pub fn encode_key(namespace: Namespace, meta: u8, key: [u8; 24]) -> U256 {
        let prefix = NamespaceCodec::encode(namespace);
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&prefix);
        bytes[PREFIX_LEN] = meta;
        bytes[PREFIX_LEN + 1..].copy_from_slice(&key);
        U256::from_be_bytes(bytes)
    }

    /// Selects a meta while preserving the prefix and instance key.
    pub(in crate::eu) fn with_field(key: U256, meta: u8) -> U256 {
        let mut bytes = key.to_be_bytes::<32>();
        bytes[PREFIX_LEN] = meta;
        U256::from_be_bytes(bytes)
    }

    pub fn decode(key: U256) -> Option<(Namespace, u8, [u8; 24])> {
        let bytes = key.to_be_bytes::<32>();
        let (namespace, payload) = NamespaceCodec::decode(&bytes)?;
        let (&meta, key) = payload.split_first()?;
        Some((namespace, meta, key.try_into().ok()?))
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
            for meta in u8::MIN..=u8::MAX {
                for key in [[0; 24], [0xff; 24]] {
                    let slot = SlotKeyCodec::encode_key(namespace, meta, key);
                    assert_eq!(SlotKeyCodec::decode(slot), Some((namespace, meta, key)));
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
                let slot = SlotKeyCodec::encode(api, 2, handle);
                let (namespace, meta, key) = SlotKeyCodec::decode(slot).unwrap();
                assert_eq!(namespace, api.into());
                assert_eq!(meta, 2);
                assert_eq!(&key[..4], &[0; 4]);
                assert_eq!(&key[4..], handle.as_slice());
                assert!(ApiAddress::is_storage_key(slot));
            }
        }
    }

    #[test]
    fn changing_field_preserves_prefix_and_key() {
        let slot = SlotKeyCodec::encode_key(Namespace::Meta, 0, [42; 24]);
        let changed = SlotKeyCodec::with_field(slot, 1);
        assert_eq!(
            SlotKeyCodec::decode(changed),
            Some((Namespace::Meta, 1, [42; 24]))
        );
        assert_eq!(
            &slot.to_be_bytes::<32>()[..PREFIX_LEN],
            &changed.to_be_bytes::<32>()[..PREFIX_LEN]
        );
    }

    #[test]
    fn decode_requires_a_valid_namespace_prefix() {
        for slot in [U256::ZERO, U256::MAX] {
            assert_eq!(SlotKeyCodec::decode(slot), None);
            assert!(!ApiAddress::is_storage_key(slot));
        }
        let slot = SlotKeyCodec::encode_key(Namespace::Meta, 1, [7; 24]);
        for index in 0..PREFIX_LEN {
            let mut bytes = slot.to_be_bytes::<32>();
            bytes[index] ^= 0xff;
            assert_eq!(SlotKeyCodec::decode(U256::from_be_bytes(bytes)), None);
        }
    }
}
