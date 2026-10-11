use super::{Numeric, StateError};
use crate::crdt;
use crate::crdt::Crdt;

/// The original and current states of a tracked value.
#[derive(Clone, PartialEq)]
pub struct ValueSnapshots<T0, T1> {
    pub(crate) original: T0,
    pub(crate) current: T1,
}

/// A pending update for a [`CrdtValue`]. `None` means there is no delta.
#[derive(Clone, Debug, PartialEq)]
pub enum CrdtDelta {
    Bytes(Vec<u8>),
    I64(i64),
    U64(DeltaOp<u64>),
    U256(DeltaOp<alloy_primitives::U256>),
    U64Set(Vec<DeltaOp<u64>>),
    None,
}

/// An arithmetic operation used by numeric and set CRDT deltas.
#[derive(Clone, Debug, PartialEq)]
pub enum DeltaOp<T> {
    Add(T),
    Sub(T),
}

/// A CRDT value whose payload may be borrowed.
#[derive(Clone, Debug, PartialEq)]
pub enum CrdtValue<'a> {
    Bytes(std::borrow::Cow<'a, crdt::bytes::Bytes>),
    U64Set(std::borrow::Cow<'a, crdt::u64_set::U64Set>),
    Numeric(Numeric<'a>),
    None,
}

impl<'a> CrdtValue<'a> {
    pub fn into_owned(self) -> CrdtValue<'static> {
        match self {
            Self::Bytes(value) => CrdtValue::Bytes(std::borrow::Cow::Owned(value.into_owned())),
            Self::U64Set(value) => CrdtValue::U64Set(std::borrow::Cow::Owned(value.into_owned())),
            Self::Numeric(value) => CrdtValue::Numeric(value.into_owned()),
            Self::None => CrdtValue::None,
        }
    }

    pub fn from_borrowed(value: &'a CrdtValue<'_>) -> Self {
        match value {
            Self::Bytes(value) => Self::Bytes(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U64Set(value) => Self::U64Set(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::Numeric(value) => Self::Numeric(Numeric::from_borrowed(value)),
            Self::None => Self::None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Numeric(Numeric::I64(value)) => value.value().copied(),
            _ => None,
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Self::Numeric(Numeric::U64(value)) => value.value().copied(),
            _ => None,
        }
    }

    pub fn as_u256(&self) -> Option<alloy_primitives::U256> {
        match self {
            Self::Numeric(Numeric::U256(value)) => value.value().copied(),
            _ => None,
        }
    }

    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Bytes(value) => value.value(),
            _ => None,
        }
    }

    pub fn as_u64_set(&self) -> Option<&crate::collections::delta_set::DeltaSet<u64>> {
        match self {
            Self::U64Set(value) => value.value(),
            _ => None,
        }
    }

    pub fn is_numeric(&self) -> bool {
        match self {
            Self::Numeric(_) => true,
            _ => false,
        }
    }

    pub fn is_commutative(&self) -> bool {
        match self {
            Self::Bytes(value) => value.is_commutative(),
            Self::U64Set(value) => value.is_commutative(),
            Self::Numeric(_) => true,
            Self::None => false,
        }
    }

    pub fn delta(&self) -> CrdtDelta {
        match self {
            Self::Bytes(value) => value
                .delta()
                .map_or(CrdtDelta::None, |delta| CrdtDelta::Bytes(delta.to_vec())),
            Self::U64Set(value) => value
                .delta()
                .map_or(CrdtDelta::None, |delta| CrdtDelta::U64Set(delta.to_vec())),
            Self::Numeric(value) => value.delta(),
            Self::None => CrdtDelta::None,
        }
    }

    pub fn add_delta(&mut self, delta: &CrdtDelta) -> Result<(), StateError> {
        match (self, delta) {
            (_, CrdtDelta::None) => Ok(()),
            (Self::Bytes(value), CrdtDelta::Bytes(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }

            (Self::U64Set(value), CrdtDelta::U64Set(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }

            (Self::Numeric(value), delta) => value.add_delta(delta),

            _ => Err(StateError::TypeMismatch),
        }
    }

    pub fn apply_delta(&mut self) -> &CrdtValue<'a> {
        match self {
            Self::Bytes(value) => {
                value.to_mut().apply_delta();
                self
            }

            Self::U64Set(value) => {
                value.to_mut().apply_delta();
                self
            }

            Self::Numeric(value) => {
                value.apply_delta();
                self
            }
            Self::None => self,
        }
    }

    pub fn applied(&self) -> Self {
        let mut applied = self.clone();
        applied.apply_delta();
        applied
    }
}
