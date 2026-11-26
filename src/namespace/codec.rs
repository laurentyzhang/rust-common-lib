use super::Namespace;
use alloy_primitives::U256;

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

    /// Returns the registered namespace and an owned 25-byte payload.
    /// Rejects invalid markers, versions, reserved bytes, and unknown namespaces.
    pub fn decode(key: U256) -> Option<(Namespace, [u8; 25])> {
        let bytes = key.to_be_bytes::<32>();
        if &bytes[..4] != b"ARCO" || bytes[4] != VERSION || bytes[6] != 0 {
            return None;
        }
        let namespace = Namespace::from_repr(bytes[5])?;
        let payload = bytes[PREFIX_LEN..].try_into().ok()?;
        Some((namespace, payload))
    }
}

#[cfg(test)]
#[path = "tests/codec.rs"]
mod tests;
