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

#[cfg(test)]
mod tests {
    use super::Bytes;
    use crate::crdt::crdt::{CacheableCrdt, Crdt};

    #[test]
    fn default_has_no_value_or_delta() {
        let bytes = Bytes::default();

        assert_eq!(bytes.value(), None);
        assert_eq!(bytes.delta.as_deref(), None);
        assert_eq!(bytes.limits(), None);
        assert!(!bytes.is_numeric());
        assert!(!bytes.is_commutative());
    }

    #[test]
    fn delta_is_the_current_value() {
        let mut bytes = Bytes::default();

        bytes.add_delta(&[1, 2, 3]).expect("add delta");

        assert_eq!(bytes.value(), Some(&[1, 2, 3][..]));
        assert_eq!(bytes.delta.as_deref(), Some(&[1, 2, 3][..]));
    }

    #[test]
    fn setting_delta_again_replaces_the_value() {
        let mut bytes = Bytes::default();
        bytes.add_delta(&[1, 2, 3]).expect("add delta");

        bytes.add_delta(&[4, 5]).expect("replace delta");

        assert_eq!(bytes.value(), Some(&[4, 5][..]));
        assert_eq!(bytes.delta.as_deref(), Some(&[4, 5][..]));
    }

    #[test]
    fn apply_delta_is_a_no_op() {
        let mut bytes = Bytes::default();
        bytes.add_delta(&[1, 2, 3]).expect("add delta");

        assert_eq!(bytes.apply_delta().value(), Some(&[1, 2, 3][..]));
        assert_eq!(bytes.delta.as_deref(), Some(&[1, 2, 3][..]));
    }

    #[test]
    fn empty_delta_is_preserved() {
        let mut bytes = Bytes::default();
        bytes.add_delta(&[]).expect("add empty delta");

        assert_eq!(bytes.value(), Some(&[][..]));
        assert_eq!(bytes.delta.as_deref(), Some(&[][..]));
    }

    #[test]
    fn cache_weight_counts_struct_and_payload_bytes() {
        let mut bytes = Bytes::default();
        let base = std::mem::size_of::<Bytes>();
        assert_eq!(bytes.cache_weight(), base);

        bytes.add_delta(&[1, 2, 3]).expect("add delta");
        assert_eq!(bytes.cache_weight(), base + 3);

        bytes.add_delta(&[4, 5]).expect("replace delta");
        assert_eq!(bytes.cache_weight(), base + 2);
    }
}
