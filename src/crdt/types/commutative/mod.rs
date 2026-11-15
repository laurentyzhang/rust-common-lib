pub mod scalar;
pub mod set;

#[cfg(test)]
#[path = "commutative_tests.rs"]
mod tests;

pub use scalar::{Scalar, Type};
pub use set::Set;
