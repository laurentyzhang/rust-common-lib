use alloy_primitives::U256;

use crate::namespace::{Namespace, NamespaceCodec};

pub struct CrdtKeyCodec;

impl CrdtKeyCodec {
    /// Validates the namespace prefix and returns only CRDT keys with their payload.
    pub fn decode(key: U256) -> Option<(Namespace, [u8; 25])> {
        let (namespace, payload) = NamespaceCodec::decode(key)?;
        match namespace {
            Namespace::U256 | Namespace::I256 | Namespace::Set => Some((namespace, payload)),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "tests/codec.rs"]
mod tests;
