mod scalar;
mod scalar_codec;

#[cfg(test)]
mod tests;

pub(in crate::eu) use scalar::{Property, ScalarAdapter};
pub use scalar_codec::ScalarKeyCodec;
