use std::cmp::Ordering;

use crate::planner::scheduler::ID;

#[derive(Clone)]
pub struct ExecutionOutput<K, T> {
    pub id: ID,
    pub key: K,
    pub gas_used: u64,
    pub accumulated_gas: u64,
    pub data: T,
}

impl<K, T> ExecutionOutput<K, T> {
    pub fn clone_with_data<U>(&self, data: U) -> ExecutionOutput<K, U>
    where
        K: Clone,
    {
        ExecutionOutput {
            id: self.id,
            key: self.key.clone(),
            gas_used: self.gas_used,
            accumulated_gas: self.accumulated_gas,
            data,
        }
    }

    pub fn map_data<U>(self, map: impl FnOnce(T) -> U) -> ExecutionOutput<K, U> {
        ExecutionOutput {
            id: self.id,
            key: self.key,
            gas_used: self.gas_used,
            accumulated_gas: self.accumulated_gas,
            data: map(self.data),
        }
    }
}

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
        self.id
            .generation
            .cmp(&other.id.generation)
            .then_with(|| self.key.cmp(&other.key))
            .then_with(|| self.gas_used.cmp(&other.gas_used))
            .then_with(|| self.id.global.cmp(&other.id.global))
    }
}
