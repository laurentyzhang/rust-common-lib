pub trait ReadOnlyStore<'a, K, V> {
    fn contains_key(&self, key: &K) -> bool; //If the key exists locally.
    fn get(&self, key: &K) -> Option<&V>;
    // fn borrow_from_fallback(&self, key: &K) -> Option<&'a V>;
}

pub trait WriteOnlyStore<'a, K, V>
where
    K: std::hash::Hash + Eq,
{
    fn stage(&mut self, updates: Vec<(K, V)>) -> Result<(), Error>;
    fn commit_batch(&mut self, updates: Vec<(K, V)>);
}

pub trait ExecutorStore<'a, K, V, D>: ReadOnlyStore<'a, K, V> + WriteOnlyStore<'a, K, V>
where
    K: std::hash::Hash + Eq,
{
    fn create(&mut self, key: K, value: V) -> Result<(), super::store::Error>;
    fn delete(&mut self, key: K);
    fn drain(&mut self) -> Vec<(K, Option<V>)>;
}

pub enum Error {
    ValueAlreadyExists,
    NotFound,
}
