use alloy_primitives::{Address, U256};
use rust_common_lib::{
    eu::{ApiAddress, CallHandler, ScalarKeyCodec, U256Handler},
    namespace::{Namespace, NamespaceCodec},
    planner::meta::{MetaKeyCodec, field::Property},
};
use strum::IntoEnumIterator;

#[test]
fn scalar_preserves_existing_layout() {
    let handle = Address::repeat_byte(0x37);
    for property in 0..=4 {
        let slot = ScalarKeyCodec::encode(property, handle);
        let bytes = slot.to_be_bytes::<32>();
        assert_eq!(&bytes[..7], &NamespaceCodec::encode(Namespace::Scalar));
        assert_eq!(bytes[7], property);
        assert_eq!(&bytes[8..12], &[0; 4]);
        assert_eq!(&bytes[12..], handle.as_slice());
        assert_eq!(
            ScalarKeyCodec::decode(slot),
            Some((property, bytes[8..].try_into().unwrap()))
        );
        for instance in [[0; 24], [0xff; 24]] {
            assert_eq!(
                ScalarKeyCodec::decode(ScalarKeyCodec::encode_key(property, instance)),
                Some((property, instance))
            );
        }
    }
}

#[test]
fn meta_encodes_selector_before_property() {
    let host = Address::from_slice(&[
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
    ]);
    for selector in [[0; 4], [0x12, 0x34, 0x56, 0x78], [0xff; 4]] {
        let mut callee = [0; 8];
        callee[..4].copy_from_slice(&host[..4]);
        callee[4..].copy_from_slice(&selector);
        let callee = u64::from_le_bytes(callee);
        for property in 0..=4 {
            for index in [0, 42, u32::MAX] {
                let slot = MetaKeyCodec::encode(property, host, callee, index);
                let bytes = slot.to_be_bytes::<32>();
                assert_eq!(&bytes[..7], &NamespaceCodec::encode(Namespace::Meta));
                assert_eq!(&bytes[7..11], &selector);
                assert_eq!(bytes[11], property);
                assert_eq!(&bytes[12..16], &index.to_be_bytes());
                assert_eq!(&bytes[16..], &host[..16]);
                assert_eq!(
                    MetaKeyCodec::decode(slot),
                    Some((property, selector, index, host[..16].try_into().unwrap()))
                );
            }
        }
        assert_eq!(
            Property::ConflictIndex.slot(host, callee, 42),
            MetaKeyCodec::encode(2, host, callee, 42)
        );
    }
}

#[test]
fn codecs_reject_other_namespaces_and_invalid_properties() {
    let scalar = ScalarKeyCodec::encode(2, Address::ZERO);
    let meta = MetaKeyCodec::encode(2, Address::ZERO, 0, 0);
    for namespace in Namespace::iter() {
        let mut bytes = scalar.to_be_bytes::<32>();
        bytes[..7].copy_from_slice(&NamespaceCodec::encode(namespace));
        assert_eq!(
            ScalarKeyCodec::decode(U256::from_be_bytes(bytes)).is_some(),
            namespace == Namespace::Scalar
        );
        let mut bytes = meta.to_be_bytes::<32>();
        bytes[..7].copy_from_slice(&NamespaceCodec::encode(namespace));
        assert_eq!(
            MetaKeyCodec::decode(U256::from_be_bytes(bytes)).is_some(),
            namespace == Namespace::Meta
        );
    }
    for property in 5..=u8::MAX {
        assert!(ScalarKeyCodec::decode(ScalarKeyCodec::encode(property, Address::ZERO)).is_none());
        assert!(
            MetaKeyCodec::decode(MetaKeyCodec::encode(property, Address::ZERO, 0, 0)).is_none()
        );
    }
    for index in [0, 1, 2, 3, 4, 6] {
        for slot in [scalar, meta] {
            let mut bytes = slot.to_be_bytes::<32>();
            bytes[index] ^= 0xff;
            let invalid = U256::from_be_bytes(bytes);
            assert!(ScalarKeyCodec::decode(invalid).is_none());
            assert!(MetaKeyCodec::decode(invalid).is_none());
        }
    }
}

#[test]
fn api_routing_uses_only_the_shared_prefix() {
    let scalar = ScalarKeyCodec::encode(2, Address::ZERO);
    let meta = Property::ConflictCount.slot(Address::ZERO, 0, 0);
    assert!(ApiAddress::is_storage_key(scalar));
    assert!(U256Handler.is_valid_slot(scalar));
    assert!(!ApiAddress::is_storage_key(meta));
    assert!(!U256Handler.is_valid_slot(meta));
    assert!(ScalarKeyCodec::decode(meta).is_none());
    assert!(MetaKeyCodec::decode(scalar).is_none());
}
