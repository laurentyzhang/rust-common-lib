use super::{Scalar, Type};
use crate::crdt::state::{CrdtValue, DeltaOp, Numeric, NumericError, StateError};
use std::borrow::Cow;

impl Type for i64 {
    type Magnitude = u64;

    const ZERO: Self = 0;
    const MIN: Self = i64::MIN;
    const MAX: Self = i64::MAX;

    fn checked_apply(value: Self, operation: &DeltaOp<Self::Magnitude>) -> Option<Self> {
        let value = i128::from(value);
        let projected = match operation {
            DeltaOp::Add(delta) => value.checked_add(i128::from(*delta))?,
            DeltaOp::Sub(delta) => value.checked_sub(i128::from(*delta))?,
        };
        i64::try_from(projected).ok()
    }

    fn magnitude_between(larger: Self, smaller: Self) -> Self::Magnitude {
        larger.abs_diff(smaller)
    }

    fn state_error(error: NumericError) -> StateError {
        StateError::I64(error)
    }
}

impl From<Scalar<i64>> for CrdtValue<'static> {
    fn from(value: Scalar<i64>) -> Self {
        CrdtValue::Numeric(Numeric::I64(Cow::Owned(value)))
    }
}
