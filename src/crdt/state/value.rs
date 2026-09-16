use super::{Delta, Numeric, StateError};
use crate::crdt::Crdt;

#[derive(Clone, PartialEq)]
pub enum Value<'a> {
    Bytes(std::borrow::Cow<'a, crate::crdt::bytes::Bytes>),
    PathMeta(std::borrow::Cow<'a, crate::crdt::path_meta::PathMeta>),
    Numeric(Numeric<'a>),
    None,
}

impl<'a> Value<'a> {
    pub fn borrowed(value: &'a Value<'_>) -> Self {
        match value {
            Self::Bytes(value) => Self::Bytes(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::PathMeta(value) => Self::PathMeta(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::Numeric(value) => Self::Numeric(Numeric::borrowed(value)),
            Self::None => Self::None,
        }
    }

    pub fn applied(&self) -> Self {
        let mut applied = self.clone();
        applied.apply_delta();
        applied
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

    pub fn as_path(&self) -> Option<&crate::collections::delta_set::DeltaSet<u64>> {
        match self {
            Self::PathMeta(value) => value.value(),
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
            Self::PathMeta(value) => value.is_commutative(),
            Self::Numeric(_) => true,
            Self::None => false,
        }
    }

    pub fn add_delta(&mut self, delta: &Delta) -> Result<(), StateError> {
        match (self, delta) {
            (_, Delta::None) => Ok(()),
            (Self::Bytes(value), Delta::Bytes(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }

            (Self::PathMeta(value), Delta::PathMeta(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }

            (Self::Numeric(value), delta) => value.add_delta(delta),

            (Self::None, _) => Err(StateError::None),
            _ => Err(StateError::TypeMismatch),
        }
    }

    pub fn apply_delta(&mut self) -> &Value<'a> {
        match self {
            Self::Bytes(value) => {
                value.to_mut().apply_delta();
                self
            }

            Self::PathMeta(value) => {
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
}
