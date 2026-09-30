use super::{Delta, StateError};
use crate::crdt::Crdt;
use std::cmp::Ordering;

#[derive(Clone, PartialEq)]
pub enum Numeric<'a> {
    I64(std::borrow::Cow<'a, crate::crdt::int64::I64>),
    U64(std::borrow::Cow<'a, crate::crdt::uint64::U64>),
    U256(std::borrow::Cow<'a, crate::crdt::u256::U256>),
}

pub(crate) struct ComparableNumeric<'a> {
    value: &'a Numeric<'static>,
}

impl<'a> ComparableNumeric<'a> {
    pub(crate) fn new(value: &'a Numeric<'static>) -> Self {
        Self { value }
    }

    pub(crate) fn compare(&self, other: &Self, tie_breaker: impl FnOnce() -> Ordering) -> Ordering {
        let ordering = match (self.value, other.value) {
            (Numeric::I64(left), Numeric::I64(right)) => left.compare(right),
            (Numeric::U64(left), Numeric::U64(right)) => left.compare(right),
            (Numeric::U256(left), Numeric::U256(right)) => left.compare(right),
            _ => unreachable!("a key cannot have different numeric types"),
        };

        ordering.then_with(tie_breaker)
    }
}

impl<'a> Numeric<'a> {
    pub fn into_owned(self) -> Numeric<'static> {
        match self {
            Self::I64(value) => Numeric::I64(std::borrow::Cow::Owned(value.into_owned())),
            Self::U64(value) => Numeric::U64(std::borrow::Cow::Owned(value.into_owned())),
            Self::U256(value) => Numeric::U256(std::borrow::Cow::Owned(value.into_owned())),
        }
    }

    pub fn from_borrowed(value: &'a Numeric<'_>) -> Self {
        match value {
            Self::I64(value) => Self::I64(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U64(value) => Self::U64(std::borrow::Cow::Borrowed(value.as_ref())),
            Self::U256(value) => Self::U256(std::borrow::Cow::Borrowed(value.as_ref())),
        }
    }

    pub fn delta(&self) -> Delta {
        match self {
            Self::I64(value) => value
                .delta()
                .map_or(Delta::None, |delta| Delta::I64(*delta)),
            Self::U64(value) => value
                .delta()
                .map_or(Delta::None, |delta| Delta::U64(delta.clone())),
            Self::U256(value) => value
                .delta()
                .map_or(Delta::None, |delta| Delta::U256(delta.clone())),
        }
    }

    pub fn delta_abs_u64(&self) -> u64 {
        match self {
            Self::I64(value) => value.delta_abs_u64(),
            Self::U64(value) => value.delta_abs_u64(),
            Self::U256(value) => value.delta_abs_u64(),
        }
    }

    pub fn delta_abs_u256(&self) -> alloy_primitives::U256 {
        match self {
            Self::I64(value) => value.delta_abs_u256(),
            Self::U64(value) => value.delta_abs_u256(),
            Self::U256(value) => value.delta_abs_u256(),
        }
    }

    pub fn add_delta(&mut self, delta: &Delta) -> Result<(), StateError> {
        match (self, delta) {
            (_, Delta::None) => Ok(()),
            (Self::I64(value), Delta::I64(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            (Self::U64(value), Delta::U64(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            (Self::U256(value), Delta::U256(delta)) => value.to_mut().add_delta(delta).map(|_| ()),
            _ => Err(StateError::TypeMismatch),
        }
    }

    pub fn apply_delta(&mut self) -> &Self {
        match self {
            Self::I64(value) => {
                value.to_mut().apply_delta();
            }
            Self::U64(value) => {
                value.to_mut().apply_delta();
            }
            Self::U256(value) => {
                value.to_mut().apply_delta();
            }
        };
        self
    }
}
