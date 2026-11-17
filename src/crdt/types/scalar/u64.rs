use super::Type;
use crate::crdt::state::{CrdtError, DeltaOp, NumericError};

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

    fn distance(larger: Self, smaller: Self) -> Self::Magnitude {
        larger - smaller
    }

    fn state_error(error: NumericError) -> CrdtError {
        CrdtError::U64(error)
    }
}
