use crate::crdt::state::{DeltaOp, NumericError, StateError};

/// A value type that can be stored in a [`super::Scalar`] CRDT.
pub trait Type: Copy + std::fmt::Debug + std::fmt::Display + Ord {
    /// The unsigned magnitude carried by an arithmetic operation.
    type Magnitude: Copy + std::fmt::Debug + std::fmt::Display + Ord + Default;

    const ZERO: Self;
    const MIN: Self;
    const MAX: Self;

    fn checked_apply(value: Self, operation: &DeltaOp<Self::Magnitude>) -> Option<Self>;
    fn magnitude_between(larger: Self, smaller: Self) -> Self::Magnitude;

    fn delta_between(base: Self, projected: Self) -> DeltaOp<Self::Magnitude> {
        if projected < base {
            DeltaOp::Sub(Self::magnitude_between(base, projected))
        } else {
            DeltaOp::Add(Self::magnitude_between(projected, base))
        }
    }

    fn state_error(error: NumericError) -> StateError;
}
