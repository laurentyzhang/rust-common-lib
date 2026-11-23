//! Crate-wide byte encoding traits, implemented by any type that is stored or shared as bytes.

use std::fmt;

use alloy_primitives::U256;

/// Failures from encoding and decoding values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CodecError {
    /// The prefix leaves no room for the chunk index, or the chunk count exceeds it.
    KeyOverflow,
    /// The bytes or words don't form a valid value.
    Invalid,
}

impl fmt::Display for CodecError {
    /// Describes the failure.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::KeyOverflow => f.write_str("codec key overflow"),
            Self::Invalid => f.write_str("invalid encoded value"),
        }
    }
}

impl std::error::Error for CodecError {}

/// Serializes a value into bytes.
pub trait Encodable {
    /// Returns the value's byte encoding.
    fn encode(&self) -> Vec<u8>;
}

/// Rebuilds a value from the bytes `Encodable::encode` produced.
pub trait Decodable: Sized {
    /// Decodes `bytes`, returning `CodecError::Invalid` if they don't form a valid value.
    fn decode(bytes: &[u8]) -> Result<Self, CodecError>;
}

impl Encodable for u64 {
    /// Encodes as 8 big-endian bytes.
    fn encode(&self) -> Vec<u8> {
        self.to_be_bytes().to_vec()
    }
}

impl Decodable for u64 {
    /// Decodes exactly 8 big-endian bytes.
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        Ok(u64::from_be_bytes(
            bytes.try_into().map_err(|_| CodecError::Invalid)?,
        ))
    }
}

impl Encodable for U256 {
    /// Encodes as 32 big-endian bytes.
    fn encode(&self) -> Vec<u8> {
        self.to_be_bytes::<32>().to_vec()
    }
}

impl Decodable for U256 {
    /// Decodes exactly 32 big-endian bytes.
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        U256::try_from_be_slice(bytes)
            .filter(|_| bytes.len() == 32)
            .ok_or(CodecError::Invalid)
    }
}

impl<T: Encodable> Encodable for Vec<T> {
    /// Concatenates the elements' encodings.
    fn encode(&self) -> Vec<u8> {
        self.iter().flat_map(Encodable::encode).collect()
    }
}

impl Decodable for Vec<u64> {
    /// Splits `bytes` into 8-byte elements; a partial trailing element is invalid.
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() % 8 != 0 {
            return Err(CodecError::Invalid);
        }
        bytes.chunks_exact(8).map(u64::decode).collect()
    }
}

impl Decodable for Vec<U256> {
    /// Splits `bytes` into 32-byte elements; a partial trailing element is invalid.
    fn decode(bytes: &[u8]) -> Result<Self, CodecError> {
        if bytes.len() % 32 != 0 {
            return Err(CodecError::Invalid);
        }
        bytes.chunks_exact(32).map(U256::decode).collect()
    }
}
