use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::StateError;
use std::cmp::Ordering;

#[derive(Debug, PartialEq)]
pub struct NonComm<T: ?Sized> {
    pub(crate) delta: Option<Box<T>>,
}

impl<T: ?Sized> Clone for NonComm<T>
where
    Box<T>: Clone,
{
    fn clone(&self) -> Self {
        Self {
            delta: self.delta.clone(),
        }
    }
}

impl<T: ?Sized> NonComm<T> {
    pub fn new(data: impl Into<Box<T>>) -> Result<Self, StateError> {
        Ok(Self {
            delta: Some(data.into()),
        })
    }
}

impl<T: ?Sized + Ord> NonComm<T> {
    /// Orders by size in bytes first, then by `T`'s own ordering.
    /// A missing delta sorts last.
    pub fn compare(&self, other: &Self) -> Ordering {
        match (self.delta.as_deref(), other.delta.as_deref()) {
            (Some(left), Some(right)) => std::mem::size_of_val(left)
                .cmp(&std::mem::size_of_val(right))
                .then_with(|| left.cmp(right)),
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
        }
    }
}

impl<T: ?Sized> Default for NonComm<T> {
    fn default() -> Self {
        Self { delta: None }
    }
}

impl<T> Crdt<T, T> for NonComm<T>
where
    T: ?Sized + PartialEq,
    Box<T>: Clone + for<'a> From<&'a T>,
{
    type Error = StateError;

    fn value(&self) -> Option<&T> {
        self.delta.as_deref()
    }

    fn delta(&self) -> Option<&T> {
        self.delta.as_deref()
    }

    fn add_delta(&mut self, delta: &T) -> Result<&T, Self::Error> {
        self.delta = Some(Box::from(delta));
        Ok(self.delta.as_deref().unwrap())
    }

    fn apply_delta(&mut self) -> &Self {
        self
    }

    fn limits(&self) -> Option<(&T, &T)> {
        None
    }

    fn is_numeric(&self) -> bool {
        false
    }

    fn is_commutative(&self) -> bool {
        false
    }
}

impl<T> CacheableCrdt<T, T> for NonComm<T>
where
    T: ?Sized + PartialEq,
    Box<T>: Clone + for<'a> From<&'a T>,
{
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>() + self.delta.as_deref().map_or(0, std::mem::size_of_val)
    }
}
