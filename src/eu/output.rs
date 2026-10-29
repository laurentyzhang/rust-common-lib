use std::cmp::Ordering;

use crate::crdt::state::status::Status;
use crate::crdt::state::{Numeric, Tracked};

#[derive(Clone)]
pub struct ExecutionOutput<K, T> {
    pub id: u64,
    pub key: K,
    pub gas_used: u64,
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
            data,
        }
    }

    pub fn map_data<U>(self, map: impl FnOnce(T) -> U) -> ExecutionOutput<K, U> {
        ExecutionOutput {
            id: self.id,
            key: self.key,
            gas_used: self.gas_used,
            data: map(self.data),
        }
    }
}

impl<K> PartialEq for ExecutionOutput<K, Tracked<Status, Status>> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<K> Eq for ExecutionOutput<K, Tracked<Status, Status>> {}

impl<K> PartialOrd for ExecutionOutput<K, Tracked<Status, Status>> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K> Ord for ExecutionOutput<K, Tracked<Status, Status>> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.data.compare(&other.data)
    }
}

impl<K> PartialEq for ExecutionOutput<K, Tracked<Status, Numeric<'static>>> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl<K> Eq for ExecutionOutput<K, Tracked<Status, Numeric<'static>>> {}

impl<K> PartialOrd for ExecutionOutput<K, Tracked<Status, Numeric<'static>>> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K> Ord for ExecutionOutput<K, Tracked<Status, Numeric<'static>>> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.data.compare(&other.data)
    }
}
