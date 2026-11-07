mod adapter;
mod key_codec;
mod kind;

#[cfg(test)]
mod tests;

pub use adapter::{Property, ScalarAdapter};
pub use key_codec::ScalarKeyCodec;
pub use kind::ScalarKind;
