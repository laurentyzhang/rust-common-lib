mod key_codec;
mod scalar;

#[cfg(test)]
mod tests;

pub use key_codec::KeyCodec;
pub(in crate::execution::evm) use key_codec::{API_OFFSET, FIELD_OFFSET};
pub(in crate::execution::evm) use scalar::{Field, ScalarSlots};

