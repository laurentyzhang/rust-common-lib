use super::StateError;
use crate::crdt::Crdt;
use crate::crdt::{NonComm, Scalar, Set};

/// The original and current states of a tracked value.
#[derive(Clone, PartialEq)]
pub struct ValueSnapshots<T0, T1> {
    pub(crate) original: T0,
    pub(crate) current: T1,
}

impl<T0: Clone, T1: Clone> ValueSnapshots<T0, T1> {
    /// Creates snapshots by cloning the original and current states.
    pub fn from_refs(original: &T0, current: &T1) -> Self {
        Self {
            original: original.clone(),
            current: current.clone(),
        }
    }
}

/// A pending update for a [`CrdtValue`]. `None` means there is no delta.
#[derive(Clone, Debug, PartialEq)]
pub enum CrdtDelta {
    Bytes(Vec<u8>),
    I256(DeltaOp<alloy_primitives::U256>),
    U256(DeltaOp<alloy_primitives::U256>),
    U64Set(Vec<DeltaOp<u64>>),
    None,
}

/// An arithmetic operation used by numeric and set CRDT deltas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum DeltaOp<T> {
    Add(T),
    Sub(T),
}

/// A CRDT value whose payload may be borrowed.
#[derive(Clone, Debug, PartialEq)]
pub enum CrdtValue<'a> {
    Bytes(std::borrow::Cow<'a, NonComm<[u8]>>),
    U64Set(std::borrow::Cow<'a, Set<u64>>),
    I256(std::borrow::Cow<'a, Scalar<alloy_primitives::I256>>),
    U256(std::borrow::Cow<'a, Scalar<alloy_primitives::U256>>),
    None,
}

impl From<NonComm<[u8]>> for CrdtValue<'static> {
    fn from(value: NonComm<[u8]>) -> Self {
        Self::Bytes(std::borrow::Cow::Owned(value))
    }
}

impl From<Set<u64>> for CrdtValue<'static> {
    fn from(value: Set<u64>) -> Self {
        Self::U64Set(std::borrow::Cow::Owned(value))
    }
}

impl<'a> CrdtValue<'a> {
    pub fn into_owned(self) -> CrdtValue<'static> {
        match self {
            Self::Bytes(value) => CrdtValue::Bytes(std::borrow::Cow::Owned(value.into_owned())),
            Self::U64Set(value) => CrdtValue::U64Set(std::borrow::Cow::Owned(value.into_owned())),
            Self::I256(value) => CrdtValue::I256(std::borrow::Cow::Owned(value.into_owned())),
            Self::U256(value) => CrdtValue::U256(std::borrow::Cow::Owned(value.into_owned())),
            Self::None => CrdtValue::None,
        }
    }

    pub fn from_borrowed(value: &'a CrdtValue<'_>) -> Self {
        match value {
            Self::Bytes(value) => Self::Bytes(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U64Set(value) => Self::U64Set(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::I256(value) => Self::I256(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U256(value) => Self::U256(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::None => Self::None,
        }
    }

    pub fn as_i256(&self) -> Option<alloy_primitives::I256> {
        match self {
            Self::I256(value) => value.value().copied(),
            _ => None,
        }
    }

    pub fn as_u256(&self) -> Option<alloy_primitives::U256> {
        match self {
            Self::U256(value) => value.value().copied(),
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
            Self::I256(_) | Self::U256(_) => true,
            _ => false,
        }
    }

    pub fn is_commutative(&self) -> bool {
        match self {
            Self::Bytes(value) => value.is_commutative(),
            Self::U64Set(value) => value.is_commutative(),
            Self::I256(_) | Self::U256(_) => true,
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
            Self::I256(value) => value
                .delta()
                .map_or(CrdtDelta::None, |delta| CrdtDelta::I256(*delta)),
            Self::U256(value) => value
                .delta()
                .map_or(CrdtDelta::None, |delta| CrdtDelta::U256(*delta)),
            Self::None => CrdtDelta::None,
        }
    }

    pub fn has_delta(&self) -> bool {
        match self {
            Self::Bytes(value) => value.delta().is_some(),
            Self::U64Set(value) => value.delta().is_some(),
            Self::I256(value) => value.delta().is_some(),
            Self::U256(value) => value.delta().is_some(),
            Self::None => false,
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

            (Self::I256(value), CrdtDelta::I256(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }
            (Self::U256(value), CrdtDelta::U256(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }

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

            Self::I256(value) => {
                value.to_mut().apply_delta();
                self
            }
            Self::U256(value) => {
                value.to_mut().apply_delta();
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
