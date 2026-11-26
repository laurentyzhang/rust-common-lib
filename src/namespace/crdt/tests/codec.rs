use super::*;
use crate::namespace::codec::PREFIX_LEN;

#[test]
fn accepts_scalar_and_container_keys() {
    let payload = [0x42; 25];
    for namespace in [Namespace::U256, Namespace::I256, Namespace::Set] {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(namespace));
        bytes[PREFIX_LEN..].copy_from_slice(&payload);
        assert_eq!(
            CrdtKeyCodec::decode(U256::from_be_bytes(bytes)),
            Some((namespace, payload))
        );
    }
}

#[test]
fn rejects_non_crdt_and_invalid_namespace_keys() {
    for namespace in [Namespace::Meta, Namespace::Runtime, Namespace::Multiprocess] {
        let mut bytes = [0; 32];
        bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(namespace));
        assert_eq!(CrdtKeyCodec::decode(U256::from_be_bytes(bytes)), None);
    }

    assert_eq!(CrdtKeyCodec::decode(U256::ZERO), None);
    let mut bytes = [0; 32];
    bytes[..PREFIX_LEN].copy_from_slice(&NamespaceCodec::encode(Namespace::U256));
    bytes[0] = 0;
    assert_eq!(CrdtKeyCodec::decode(U256::from_be_bytes(bytes)), None);
}
