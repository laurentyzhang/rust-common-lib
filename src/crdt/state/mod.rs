//! Shared CRDT values, deltas, and access tracking.

pub mod delta;
pub mod error;
pub mod markers;
pub mod numeric;
pub mod op;
pub mod output;
pub mod tracked;
pub mod value;

pub use delta::{Delta, DeltaOp};
pub use error::{NumericError, StateError};
pub use markers::Marker;
pub use numeric::Numeric;
pub use output::TransactionOutput;
pub use tracked::Tracked;
pub use value::Value;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
