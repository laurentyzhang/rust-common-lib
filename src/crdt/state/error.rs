use std::fmt::Display;

#[derive(Debug, PartialEq, Eq)]
pub enum NumericError {
    InvalidLimits(String),
    BelowLowerLimit(String),
    AboveUpperLimit(String),
    Overflow(String),
    Underflow(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum StateError {
    I64(NumericError),
    U64(NumericError),
    U256(NumericError),
    CannotAddDeltaToMissingValue,
    TypeMismatch,
    None,
}

impl NumericError {
    pub(crate) fn invalid_limits<T: Display>(lower: &T, upper: &T) -> Self {
        Self::InvalidLimits(format!("lower limit {lower} exceeds upper limit {upper}"))
    }

    pub(crate) fn below_lower_limit<T: Display>(value: &T, lower: &T, upper: &T) -> Self {
        Self::BelowLowerLimit(format!("value {value} is below limits [{lower}, {upper}]"))
    }

    pub(crate) fn above_upper_limit<T: Display>(value: &T, lower: &T, upper: &T) -> Self {
        Self::AboveUpperLimit(format!("value {value} is above limits [{lower}, {upper}]"))
    }

    pub(crate) fn overflow<T: Display>(value: &T, pending_delta: &T, delta: &T) -> Self {
        Self::Overflow(format!(
            "operation overflowed: value {value}, pending delta {pending_delta}, incoming delta {delta}"
        ))
    }

    pub(crate) fn underflow<T: Display>(value: &T, pending_delta: &T, delta: &T) -> Self {
        Self::Underflow(format!(
            "operation underflowed: value {value}, pending delta {pending_delta}, incoming delta {delta}"
        ))
    }
}
