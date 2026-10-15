use crate::crdt::state::{CrdtDelta, CrdtValue};
use crate::crdt::{NonComm, Scalar, Set};

/// A value held by an execution cache.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry<'a, T = ()> {
    Crdt(CrdtValue<'a>),
    Other(T),
}

impl<T> Entry<'_, T> {
    pub fn is_missing(&self) -> bool {
        matches!(self, Self::Crdt(CrdtValue::None))
    }

    pub fn delta(&self) -> Option<CrdtDelta> {
        match self {
            Self::Crdt(value) => match value.delta() {
                CrdtDelta::None => None,
                delta => Some(delta),
            },
            Self::Other(_) => None,
        }
    }

    pub fn apply_delta(&mut self) {
        if let Self::Crdt(value) = self {
            value.apply_delta();
        }
    }

    pub fn applied(&self) -> Self
    where
        T: Clone,
    {
        let mut value = self.clone();
        value.apply_delta();
        value
    }

    pub fn as_crdt(&self) -> Option<&CrdtValue<'_>> {
        match self {
            Self::Crdt(value) => Some(value),
            Self::Other(_) => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        self.as_crdt()?.as_i64()
    }

    pub fn as_u64(&self) -> Option<u64> {
        self.as_crdt()?.as_u64()
    }

    pub fn as_u256(&self) -> Option<alloy_primitives::U256> {
        self.as_crdt()?.as_u256()
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        self.as_crdt()?.as_bytes()
    }

    pub fn as_u64_set(&self) -> Option<&crate::collections::delta_set::DeltaSet<u64>> {
        self.as_crdt()?.as_u64_set()
    }

    pub fn into_owned(self) -> Entry<'static, T> {
        match self {
            Self::Crdt(value) => Entry::Crdt(value.into_owned()),
            Self::Other(value) => Entry::Other(value),
        }
    }
}

impl<T> From<CrdtValue<'static>> for Entry<'static, T> {
    fn from(value: CrdtValue<'static>) -> Self {
        Self::Crdt(value)
    }
}

impl<T> From<NonComm<[u8]>> for Entry<'static, T> {
    fn from(value: NonComm<[u8]>) -> Self {
        Self::Crdt(value.into())
    }
}

impl<T> From<Set<u64>> for Entry<'static, T> {
    fn from(value: Set<u64>) -> Self {
        Self::Crdt(value.into())
    }
}

impl<T> From<Scalar<i64>> for Entry<'static, T> {
    fn from(value: Scalar<i64>) -> Self {
        Self::Crdt(value.into())
    }
}

impl<T> From<Scalar<u64>> for Entry<'static, T> {
    fn from(value: Scalar<u64>) -> Self {
        Self::Crdt(value.into())
    }
}

impl<T> From<Scalar<alloy_primitives::U256>> for Entry<'static, T> {
    fn from(value: Scalar<alloy_primitives::U256>) -> Self {
        Self::Crdt(value.into())
    }
}
