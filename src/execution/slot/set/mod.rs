#![doc = include_str!("README.md")]

mod indexer;
mod key_codec;
mod merger;
mod converter;

#[cfg(test)]
#[path = "set_tests.rs"]
mod tests;

pub use indexer::Indexer;
pub use key_codec::{KeyCodec, Property};
pub use merger::Merger;
pub use converter::Converter;
