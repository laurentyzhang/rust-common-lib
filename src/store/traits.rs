pub trait FallbackStore<'a, K, V> {
    fn contains_key(&self, key: &K) -> bool; //If the key exists locally.
    fn get(&self, key: &K) -> Option<&V>;
}

pub trait WriteOnlyStore<K, V>
where
    K: std::hash::Hash + Eq,
{
    fn stage(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError>;
    fn commit(&mut self, updates: Vec<(K, V)>);
}

// pub trait ExecutorStore<'a, K, V, D>: FallbackStore<'a, K, V> + WriteOnlyStore<K, V>
// where
//     K: std::hash::Hash + Eq,
// {
//     fn create(&mut self, key: K, value: V) -> Result<(), StoreError>;
//     fn delete(&mut self, key: K);
//     fn drain(&mut self) -> Vec<(K, Option<V>)>;
// }

#[derive(Debug, PartialEq, Eq)]
pub enum StoreError {
    ValueCannotBeRecreated,
    NotFound,
    EntryNotFound,
}
