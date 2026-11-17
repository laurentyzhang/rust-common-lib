pub mod scalar;
pub mod set;

pub use scalar::{Scalar, Type};
pub use set::Set;

#[cfg(test)]
#[path = "types_tests.rs"]
pub mod tests;
