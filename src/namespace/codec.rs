use super::Namespace;

/// Supported namespace-prefix format version.
pub const VERSION: u8 = 1;
/// Number of prefix bytes before the namespace-specific payload.
pub const PREFIX_LEN: usize = 7;

/// Encodes and validates the prefix shared by all ARCO namespaces.
///
/// Layout: ARCO(4) | version(1) | namespace(1) | reserved(1).
/// The reserved byte must be zero. Payload encoding belongs to the caller.
pub struct NamespaceCodec;

impl NamespaceCodec {
    /// Builds the prefix using the current version and a registered namespace ID.
    pub fn encode(namespace: Namespace) -> [u8; PREFIX_LEN] {
        [b'A', b'R', b'C', b'O', VERSION, namespace as u8, 0]
    }

    /// Returns the registered namespace and the remaining payload without copying it.
    /// Returns `None` for a short prefix, invalid ARCO marker, unsupported version,
    /// nonzero reserved byte, or unknown namespace ID.
    pub fn decode(encoded: &[u8]) -> Option<(Namespace, &[u8])> {
        let (prefix, payload) = encoded.split_at_checked(PREFIX_LEN)?;
        if &prefix[..4] != b"ARCO" || prefix[4] != VERSION || prefix[6] != 0 {
            return None;
        }
        Some((Namespace::from_repr(prefix[5])?, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strum::IntoEnumIterator;

    #[test]
    fn round_trip_namespaces_preserves_payload() {
        for namespace in Namespace::iter() {
            let mut encoded = NamespaceCodec::encode(namespace).to_vec();
            encoded.extend_from_slice(&[7, 42, 255]);
            assert_eq!(
                NamespaceCodec::decode(&encoded),
                Some((namespace, &[7, 42, 255][..]))
            );
        }
    }

    #[test]
    fn rejects_invalid_prefixes() {
        let valid = NamespaceCodec::encode(Namespace::Meta);
        assert_eq!(valid, [b'A', b'R', b'C', b'O', VERSION, 0xc0, 0]);
        for length in 0..PREFIX_LEN {
            assert_eq!(NamespaceCodec::decode(&valid[..length]), None);
        }
        for index in [0, 1, 2, 3, 5, 6] {
            let mut encoded = valid;
            encoded[index] ^= 0xff;
            assert_eq!(NamespaceCodec::decode(&encoded), None);
        }
        for version in u8::MIN..=u8::MAX {
            let mut encoded = valid;
            encoded[4] = version;
            assert_eq!(
                NamespaceCodec::decode(&encoded).is_some(),
                version == VERSION
            );
        }
    }
}
