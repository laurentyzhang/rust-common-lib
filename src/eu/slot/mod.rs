mod key_codec;
mod scalar;

#[cfg(test)]
mod tests;

pub use key_codec::KeyCodec;
pub(in crate::eu) use key_codec::{API_OFFSET, FIELD_OFFSET};
pub(in crate::eu) use scalar::{Field, ScalarAdapter};
