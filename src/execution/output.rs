use crate::execution::workload::ID;

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
