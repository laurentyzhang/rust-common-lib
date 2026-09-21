use crate::store::StoreError;

pub trait TransitionWriter<K, V>
where
    K: std::hash::Hash + Eq,
{
    fn flush(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError>;
}

impl<K, V, W> TransitionWriter<K, V> for &mut W
where
    K: std::hash::Hash + Eq,
    W: TransitionWriter<K, V> + ?Sized,
{
    fn flush(&mut self, updates: Vec<(K, V)>) -> Result<(), StoreError> {
        (**self).flush(updates)
    }
}
