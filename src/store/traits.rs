pub trait FallbackStore<'a, K, V> {
    fn contains_key(&self, key: &K) -> bool; //If the key exists locally.
    fn get(&self, key: &K) -> Option<&V>;
}

pub trait WriteOnlyStore<K, V>
where
    K: std::hash::Hash + Eq,
{
    fn stage(&mut self, updates: Vec<(K, V)>) -> Result<(), Error>;
    fn commit_batch(&mut self, updates: Vec<(K, V)>);
}

pub trait ExecutorStore<'a, K, V, D>: FallbackStore<'a, K, V> + WriteOnlyStore<K, V>
where
    K: std::hash::Hash + Eq,
{
    fn create(&mut self, key: K, value: V) -> Result<(), Error>;
    fn delete(&mut self, key: K);
    fn drain(&mut self) -> Vec<(K, Option<V>)>;
}

pub enum Error {
    ValueCannotBeRecreated,
    NotFound,
}
