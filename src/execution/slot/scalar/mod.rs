mod adapter;
mod key_codec;
mod kind;

#[cfg(test)]
#[path = "scalar_tests.rs"]
mod tests;

pub use adapter::{Property, ScalarAdapter};
pub use key_codec::ScalarKeyCodec;
pub use kind::ScalarKind;
