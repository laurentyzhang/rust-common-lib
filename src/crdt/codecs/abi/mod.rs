use alloy_primitives::{I256, U256};

use crate::crdt::Type;

/// Encodes a CRDT scalar value as one ABI storage word.
pub trait AbiCodec: Type {
    fn to_word(self) -> U256;
    fn from_word(word: U256) -> Option<Self>;
}

impl AbiCodec for U256 {
    fn to_word(self) -> U256 {
        self
    }

    fn from_word(word: U256) -> Option<Self> {
        Some(word)
    }
}

impl AbiCodec for I256 {
    fn to_word(self) -> U256 {
        self.into_raw()
    }

    fn from_word(word: U256) -> Option<Self> {
        Some(I256::from_raw(word))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u256_round_trip() {
        for value in [U256::ZERO, U256::from(1), U256::MAX] {
            assert_eq!(U256::from_word(value.to_word()), Some(value));
        }
    }

    #[test]
    fn i256_round_trip() {
        for value in [I256::MIN, I256::MINUS_ONE, I256::ZERO, I256::ONE, I256::MAX] {
            assert_eq!(I256::from_word(value.to_word()), Some(value));
        }
    }
}
