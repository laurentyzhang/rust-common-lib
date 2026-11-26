use super::*;
use strum::IntoEnumIterator;

#[test]
fn round_trip_namespaces_preserves_payload() {
    let payload: [u8; 25] = std::array::from_fn(|index| index as u8);
    for namespace in Namespace::iter() {
        let mut encoded = [0; 32];
        encoded[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(namespace));
        encoded[PREFIX_LEN..].copy_from_slice(&payload);
        assert_eq!(
            NamespaceCodec::decode(U256::from_be_bytes(encoded)),
            Some((namespace, payload))
        );
    }
}

#[test]
fn rejects_invalid_prefixes() {
    let prefix = NamespaceCodec::encode(Namespace::Meta);
    assert_eq!(prefix, [b'A', b'R', b'C', b'O', VERSION, 0xc0, 0]);
    let mut valid = [0; 32];
    valid[..PREFIX_LEN].copy_from_slice(&prefix);
    assert_eq!(NamespaceCodec::decode(U256::ZERO), None);
    for index in [0, 1, 2, 3, 5, 6] {
        let mut encoded = valid;
        encoded[index] ^= 0xff;
        assert_eq!(NamespaceCodec::decode(U256::from_be_bytes(encoded)), None);
    }
    for version in u8::MIN..=u8::MAX {
        let mut encoded = valid;
        encoded[4] = version;
        assert_eq!(
            NamespaceCodec::decode(U256::from_be_bytes(encoded)).is_some(),
            version == VERSION
        );
    }
}
