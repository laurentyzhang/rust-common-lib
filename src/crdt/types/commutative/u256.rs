use super::{Scalar, Type};
use crate::crdt::state::{CrdtValue, DeltaOp, Numeric, NumericError, StateError};
use alloy_primitives::U256 as AlloyU256;
use std::borrow::Cow;

impl Type for AlloyU256 {
    type Magnitude = AlloyU256;

    const ZERO: Self = AlloyU256::ZERO;
    const MIN: Self = AlloyU256::ZERO;
    const MAX: Self = AlloyU256::MAX;

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
        StateError::U256(error)
    }
}

impl From<Scalar<AlloyU256>> for CrdtValue<'static> {
    fn from(value: Scalar<AlloyU256>) -> Self {
        CrdtValue::Numeric(Numeric::U256(Cow::Owned(value)))
    }
}
