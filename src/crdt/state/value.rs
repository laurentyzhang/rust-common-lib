use super::{Delta, Marker, Numeric, StateError};
use crate::crdt::Crdt;

#[derive(Clone, PartialEq)]
pub enum Value<'a> {
    Bytes(std::borrow::Cow<'a, crate::crdt::bytes::Bytes>),
    U64Set(std::borrow::Cow<'a, crate::crdt::u64_set::U64Set>),
    Numeric(Numeric<'a>),
    Marker(Marker),
}

impl<'a> Value<'a> {
    pub fn into_owned(self) -> Value<'static> {
        match self {
            Self::Bytes(value) => Value::Bytes(std::borrow::Cow::Owned(value.into_owned())),
            Self::U64Set(value) => Value::U64Set(std::borrow::Cow::Owned(value.into_owned())),
            Self::Numeric(value) => Value::Numeric(value.into_owned()),
            Self::Marker(marker) => Value::Marker(marker),
        }
    }

    pub fn from_borrowed(value: &'a Value<'_>) -> Self {
        match value {
            Self::Bytes(value) => Self::Bytes(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U64Set(value) => Self::U64Set(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::Numeric(value) => Self::Numeric(Numeric::from_borrowed(value)),
            Self::Marker(marker) => Self::Marker(*marker),
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
            Self::Marker(_) => false,
        }
    }

    pub fn delta(&self) -> Delta {
        match self {
            Self::Bytes(value) => value
                .delta()
                .map_or(Delta::None, |delta| Delta::Bytes(delta.to_vec())),
            Self::U64Set(value) => value
                .delta()
                .map_or(Delta::None, |delta| Delta::U64Set(delta.to_vec())),
            Self::Numeric(value) => value.delta(),
            Self::Marker(_) => Delta::None,
        }
    }

    pub fn add_delta(&mut self, delta: &Delta) -> Result<(), StateError> {
        match (self, delta) {
            (_, Delta::None) => Ok(()),
            (Self::Bytes(value), Delta::Bytes(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }

            (Self::U64Set(value), Delta::U64Set(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }

            (Self::Numeric(value), delta) => value.add_delta(delta),

            _ => Err(StateError::TypeMismatch),
        }
    }

    pub fn apply_delta(&mut self) -> &Value<'a> {
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
            Self::Marker(_) => self,
        }
    }

    pub fn applied(&self) -> Self {
        let mut applied = self.clone();
        applied.apply_delta();
        applied
    }
}
