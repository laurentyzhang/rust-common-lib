//! Shared CRDT values, deltas, and access tracking.

pub mod delta;
pub mod error;
pub mod numeric;
pub mod op;
pub mod status;
pub mod tracked;
pub mod value;

pub use delta::{Delta, DeltaOp};
pub use error::{NumericError, StateError};
pub use numeric::Numeric;
pub use status::Status;
pub use tracked::Tracked;
pub use value::Value;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
