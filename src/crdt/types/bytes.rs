use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::{StateError, Value};
use std::borrow::Cow;

#[derive(Clone, PartialEq, Default)]
pub struct Bytes {
    pub(crate) delta: Option<Box<[u8]>>,
}

impl From<Bytes> for Value<'static> {
    fn from(value: Bytes) -> Self {
        Value::Bytes(Cow::Owned(value))
    }
}

impl Bytes {
    pub fn new(data: Vec<u8>) -> Result<Self, StateError> {
        Ok(Self {
            delta: Some(data.into_boxed_slice()),
        })
    }
}

impl Crdt<[u8], [u8]> for Bytes {
    type Error = StateError;

    fn value(&self) -> Option<&[u8]> {
        self.delta.as_deref()
    }

    fn add_delta(&mut self, delta: &[u8]) -> Result<&[u8], Self::Error> {
        let stored = self.delta.insert(delta.into());
        Ok(stored)
    }

    fn apply_delta(&mut self) -> &Self {
        self
    }

    fn limits(&self) -> Option<(&[u8], &[u8])> {
        None
    }

    fn is_numeric(&self) -> bool {
        false
    }

    fn is_commutative(&self) -> bool {
        false
    }
}

impl CacheableCrdt<[u8], [u8]> for Bytes {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>() + self.delta.as_deref().map_or(0, <[u8]>::len)
    }
}
