use super::{Scalar, Type};
use crate::crdt::state::{CrdtValue, DeltaOp, Numeric, NumericError, StateError};
use alloy_primitives::{I256, U256};
use std::borrow::Cow;

impl Type for I256 {
    type Magnitude = U256;

    const ZERO: Self = I256::ZERO;
    const MIN: Self = I256::MIN;
    const MAX: Self = I256::MAX;

    fn checked_apply(value: Self, operation: &DeltaOp<Self::Magnitude>) -> Option<Self> {
        let sign = U256::from(1) << 255;
        let ordered = value.into_raw() ^ sign;
        let projected = match operation {
            DeltaOp::Add(delta) => ordered.checked_add(*delta)?,
            DeltaOp::Sub(delta) => ordered.checked_sub(*delta)?,
        };
        Some(I256::from_raw(projected ^ sign))
    }

    fn magnitude_between(larger: Self, smaller: Self) -> Self::Magnitude {
        let sign = U256::from(1) << 255;
        (larger.into_raw() ^ sign) - (smaller.into_raw() ^ sign)
    }

    fn state_error(error: NumericError) -> StateError {
        StateError::I256(error)
    }
}

impl From<Scalar<I256>> for CrdtValue<'static> {
    fn from(value: Scalar<I256>) -> Self {
        CrdtValue::Numeric(Numeric::I256(Cow::Owned(value)))
    }
}
