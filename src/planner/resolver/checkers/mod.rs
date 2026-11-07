pub mod access;
pub mod sum;
pub mod util;

use crate::execution::output::ExecutionOutput;
use std::cmp::Ordering;

impl<K: Ord, T> PartialEq for ExecutionOutput<K, T> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<K: Ord, T> Eq for ExecutionOutput<K, T> {}

impl<K: Ord, T> PartialOrd for ExecutionOutput<K, T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Ord, T> Ord for ExecutionOutput<K, T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key
            .cmp(&other.key)
            .then_with(|| self.gas_used.cmp(&other.gas_used))
            .then_with(|| self.id.block.cmp(&other.id.block))
            .then_with(|| self.id.global.cmp(&other.id.global))
            .then_with(|| self.id.generation.cmp(&other.id.generation))
    }
}
