mod codec;
pub mod scalar;
pub mod set;
mod traits;

pub use codec::Codec;
pub use traits::{FormatConverter, DeltaMerger};

/// Reports a store failure as a database error.
pub(in crate::execution::slot) fn database<E: std::fmt::Debug>(error: E) -> crate::store::StoreError {
    crate::store::StoreError::Database(format!("{error:?}"))
}
