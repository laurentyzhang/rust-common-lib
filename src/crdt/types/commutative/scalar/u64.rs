use super::{Scalar, Type};
use crate::crdt::state::{CrdtValue, DeltaOp, Numeric, NumericError, StateError};
use std::borrow::Cow;

impl Type for u64 {
    type Magnitude = u64;

    const ZERO: Self = 0;
    const MIN: Self = u64::MIN;
    const MAX: Self = u64::MAX;

    fn checked_apply(value: Self, operation: &DeltaOp<Self::Magnitude>) -> Option<Self> {
        match operation {
            DeltaOp::Add(delta) => value.checked_add(*delta),
            DeltaOp::Sub(delta) => value.checked_sub(*delta),
        }
    }

    fn magnitude_between(larger: Self, smaller: Self) -> Self::Magnitude {
        larger - smaller
    }

    fn state_error(error: NumericError) -> StateError {
        StateError::U64(error)
    }
}

impl From<Scalar<u64>> for CrdtValue<'static> {
    fn from(value: Scalar<u64>) -> Self {
        CrdtValue::Numeric(Numeric::U64(Cow::Owned(value)))
    }
}
