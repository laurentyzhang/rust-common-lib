pub mod key;
mod traits;

pub use traits::{AccountStorage, ApiStore};

#[cfg(test)]
#[path = "storage_tests.rs"]
mod tests;
