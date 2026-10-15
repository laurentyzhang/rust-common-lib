use super::{CrdtDelta, StateError};
use crate::crdt::Crdt;
use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq)]
pub enum Numeric<'a> {
    I64(std::borrow::Cow<'a, crate::crdt::Scalar<i64>>),
    U64(std::borrow::Cow<'a, crate::crdt::Scalar<u64>>),
    U256(std::borrow::Cow<'a, crate::crdt::Scalar<alloy_primitives::U256>>),
}

pub(crate) struct ComparableNumeric<'a, 'value> {
    value: &'a Numeric<'value>,
}

impl<'a, 'value> ComparableNumeric<'a, 'value> {
    pub(crate) fn new(value: &'a Numeric<'value>) -> Self {
        Self { value }
    }

    pub(crate) fn compare(
        &self,
        other: &ComparableNumeric<'_, 'value>,
        tie_breaker: impl FnOnce() -> Ordering,
    ) -> Ordering {
        let ordering = match (self.value, other.value) {
            (Numeric::I64(left), Numeric::I64(right)) => left.compare(right),
            (Numeric::U64(left), Numeric::U64(right)) => left.compare(right),
            (Numeric::U256(left), Numeric::U256(right)) => left.compare(right),
            _ => unreachable!("a key cannot have different numeric types"),
        };

        ordering.then_with(tie_breaker)
    }
}

impl Eq for Numeric<'_> {}

impl PartialOrd for Numeric<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Numeric<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::I64(left), Self::I64(right)) => {
                ComparableNumeric::new(self).compare(&ComparableNumeric::new(other), || {
                    left.delta.cmp(&right.delta).then_with(|| {
                        left.limits
                            .cmp(&right.limits)
                            .then_with(|| left.value.cmp(&right.value))
                    })
                })
            }
            (Self::U64(left), Self::U64(right)) => {
                ComparableNumeric::new(self).compare(&ComparableNumeric::new(other), || {
                    left.delta
                        .cmp(&right.delta)
                        .then_with(|| left.limits.cmp(&right.limits))
                        .then_with(|| left.value.cmp(&right.value))
                })
            }
            (Self::U256(left), Self::U256(right)) => {
                ComparableNumeric::new(self).compare(&ComparableNumeric::new(other), || {
                    left.delta
                        .cmp(&right.delta)
                        .then_with(|| left.limits.cmp(&right.limits))
                        .then_with(|| left.value.cmp(&right.value))
                })
            }
            (Self::I64(_), Self::U64(_) | Self::U256(_)) | (Self::U64(_), Self::U256(_)) => {
                Ordering::Less
            }
            (Self::U64(_) | Self::U256(_), Self::I64(_)) | (Self::U256(_), Self::U64(_)) => {
                Ordering::Greater
            }
        }
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

    pub fn delta(&self) -> CrdtDelta {
        match self {
            Self::I64(value) => value
                .delta()
                .map_or(CrdtDelta::None, |delta| CrdtDelta::I64(delta.clone())),
            Self::U64(value) => value
                .delta()
                .map_or(CrdtDelta::None, |delta| CrdtDelta::U64(delta.clone())),
            Self::U256(value) => value
                .delta()
                .map_or(CrdtDelta::None, |delta| CrdtDelta::U256(delta.clone())),
        }
    }

    pub fn add_delta(&mut self, delta: &CrdtDelta) -> Result<(), StateError> {
        match (self, delta) {
            (_, CrdtDelta::None) => Ok(()),
            (Self::I64(value), CrdtDelta::I64(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }
            (Self::U64(value), CrdtDelta::U64(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }
            (Self::U256(value), CrdtDelta::U256(delta)) => {
                value.to_mut().add_delta(delta).map(|_| ())
            }
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
