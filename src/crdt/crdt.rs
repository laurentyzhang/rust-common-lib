pub trait Crdt<T: ?Sized + PartialEq, D: ?Sized + PartialEq>: Clone + PartialEq + Default {
    type Error;

    fn value(&self) -> Option<&T>;
    fn add_delta(&mut self, delta: &D) -> Result<&D, Self::Error>;
    fn apply_delta(&mut self) -> &Self;
    fn limits(&self) -> Option<(&T, &T)>;

    fn is_numeric(&self) -> bool;
    fn is_commutative(&self) -> bool;
}

pub trait CacheableCrdt<T: ?Sized + PartialEq, D: ?Sized + PartialEq>: Crdt<T, D> {
    fn cache_weight(&self) -> usize;
}
