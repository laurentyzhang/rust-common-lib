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
#[path = "tests/abi.rs"]
mod tests;
