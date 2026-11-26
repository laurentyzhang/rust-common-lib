pub mod collections;
pub mod scalar;

pub use collections::Set;
pub use scalar::{Scalar, Type};

#[cfg(test)]
#[path = "tests/types.rs"]
pub mod tests;
