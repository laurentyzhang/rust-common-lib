use alloy_primitives::U256;

use crate::common::codec::{CodecError, Decodable, Encodable};
use crate::common::width::Width;

/// Splits a value's bytes into words stored over a run of slots.
/// Key layout: [prefix][index][suffix], the index taking the bytes left over. Index 0 holds the
/// byte length; indexes 1.. hold the 32-byte chunks, the last zero-padded.
pub struct Codec {
    prefix: Vec<u8>,
    suffix: Vec<u8>,
}

impl Codec {
    /// Creates a codec whose keys start with `prefix` and end with `suffix`.
    pub fn new(prefix: Vec<u8>, suffix: Vec<u8>) -> Self {
        Self { prefix, suffix }
    }

    /// Encodes `value`, writing its length and then all its chunks through `write`.
    /// Write errors pass through unchanged; codec failures convert into the caller's error type.
    pub fn encode<T: Encodable, E: From<CodecError>>(
        &self,
        value: &T,
        write: impl FnMut(U256, U256) -> Result<(), E>,
    ) -> Result<(), E> {
        self.write_from(&value.encode(), 0, write)
    }

    /// Encodes `value` but writes only its length and the chunks from element `at` onward,
    /// where elements are `U`, `N` bytes each; for updates that change only a suffix.
    pub fn encode_at<T: Encodable, const N: usize, U: Width<N>, E: From<CodecError>>(
        &self,
        value: &T,
        at: usize,
        write: impl FnMut(U256, U256) -> Result<(), E>,
    ) -> Result<(), E> {
        self.write_from(&value.encode(), at * N, write)
    }

    /// Decodes a `T` stored under the codec, reading its length and then its chunks through `read`.
    /// Read errors pass through unchanged; codec failures convert into the caller's error type.
    pub fn decode<T: Decodable, E: From<CodecError>>(
        &self,
        mut read: impl FnMut(U256) -> Result<U256, E>,
    ) -> Result<T, E> {
        let len = usize::try_from(read(self.key(0)?)?).map_err(|_| E::from(CodecError::Invalid))?;
        let mut bytes = Vec::with_capacity(len.next_multiple_of(32));
        for i in 0..len.div_ceil(32) {
            bytes.extend_from_slice(&read(self.key(i as u64 + 1)?)?.to_be_bytes::<32>());
        }
        bytes.truncate(len);
        Ok(T::decode(&bytes).map_err(|_| E::from(CodecError::Invalid))?)
    }

    /// Decodes element `at` of a stored sequence of `T`, reading only the length and the
    /// chunks that hold it. An element past the stored length is invalid.
    pub fn decode_at<const N: usize, T: Decodable + Width<N>, E: From<CodecError>>(
        &self,
        at: usize,
        mut read: impl FnMut(U256) -> Result<U256, E>,
    ) -> Result<T, E> {
        let len = usize::try_from(read(self.key(0)?)?).map_err(|_| E::from(CodecError::Invalid))?;
        let (start, end) = (at * N, (at + 1) * N);
        if end > len {
            return Err(CodecError::Invalid.into());
        }

        // An element may straddle chunks, so read every chunk it touches.
        let first = start / 32;
        let mut bytes = Vec::with_capacity((end.div_ceil(32) - first) * 32);
        for i in first..end.div_ceil(32) {
            bytes.extend_from_slice(&read(self.key(i as u64 + 1)?)?.to_be_bytes::<32>());
        }
        Ok(T::decode(&bytes[start - first * 32..end - first * 32])?)
    }

    /// Builds the full key for chunk `index` (0 is the length), rejecting keys that overflow 32 bytes.
    pub fn key(&self, index: u64) -> Result<U256, CodecError> {
        let width = 32usize
            .checked_sub(self.prefix.len() + self.suffix.len())
            .filter(|&width| width > 0)
            .ok_or(CodecError::KeyOverflow)?;
        if width < 8 && index >> (width * 8) != 0 {
            return Err(CodecError::KeyOverflow);
        }

        let mut key = [0; 32];
        let end = self.prefix.len() + width;
        key[..self.prefix.len()].copy_from_slice(&self.prefix);
        key[end..].copy_from_slice(&self.suffix);
        let n = width.min(8);
        key[end - n..end].copy_from_slice(&index.to_be_bytes()[8 - n..]);
        Ok(U256::from_be_bytes(key))
    }

    /// Writes the length of `bytes` and its chunks from the one holding byte `from` onward.
    fn write_from<E: From<CodecError>>(
        &self,
        bytes: &[u8],
        from: usize,
        mut write: impl FnMut(U256, U256) -> Result<(), E>,
    ) -> Result<(), E> {
        write(self.key(0)?, U256::from(bytes.len()))?;
        for (i, chunk) in bytes.chunks(32).enumerate().skip(from / 32) {
            let mut word = [0; 32];
            word[..chunk.len()].copy_from_slice(chunk);
            write(self.key(i as u64 + 1)?, U256::from_be_bytes(word))?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "codec_tests.rs"]
mod tests;
