use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::Numeric;
use crate::crdt::state::Value;
use crate::crdt::state::{NumericError, StateError};
use std::borrow::Cow;
#[derive(Clone, PartialEq)]
pub struct I64 {
    pub(crate) value: i64,
    pub(crate) delta: i64,
    pub(crate) limits: (i64, i64),
}

impl Default for I64 {
    fn default() -> Self {
        Self {
            value: 0,
            delta: 0,
            limits: (i64::MIN, i64::MAX),
        }
    }
}

impl From<I64> for Value<'static> {
    fn from(value: I64) -> Self {
        Value::Numeric(Numeric::I64(Cow::Owned(value)))
    }
}

impl I64 {
    pub fn new(lower: i64, upper: i64) -> Result<Self, StateError> {
        Self::check_limits(lower, upper, 0)?;
        Ok(Self {
            value: 0,
            delta: 0,
            limits: (lower, upper),
        })
    }

    fn check_limits(lower: i64, upper: i64, value: i64) -> Result<(), StateError> {
        if lower > upper {
            Err(StateError::I64(NumericError::invalid_limits(
                &lower, &upper,
            )))
        } else if value < lower {
            Err(StateError::I64(NumericError::below_lower_limit(
                &value, &lower, &upper,
            )))
        } else if value > upper {
            Err(StateError::I64(NumericError::above_upper_limit(
                &value, &lower, &upper,
            )))
        } else {
            Ok(())
        }
    }
}

impl Crdt<i64, i64> for I64 {
    type Error = StateError;

    fn value(&self) -> Option<&i64> {
        Some(&self.value)
    }

    fn delta(&self) -> Option<&i64> {
        Some(&self.delta)
    }

    fn add_delta(&mut self, delta: &i64) -> Result<&i64, Self::Error> {
        let accumulated = self.delta.checked_add(*delta).ok_or_else(|| {
            if *delta < 0 {
                StateError::I64(NumericError::underflow(&self.value, &self.delta, delta))
            } else {
                StateError::I64(NumericError::overflow(&self.value, &self.delta, delta))
            }
        })?;
        let projected = self.value.checked_add(accumulated).ok_or_else(|| {
            if accumulated < 0 {
                StateError::I64(NumericError::underflow(&self.value, &self.delta, delta))
            } else {
                StateError::I64(NumericError::overflow(&self.value, &self.delta, delta))
            }
        })?;

        let (lower, upper) = self.limits;
        if projected < lower {
            return Err(StateError::I64(NumericError::below_lower_limit(
                &projected, &lower, &upper,
            )));
        }
        if projected > upper {
            return Err(StateError::I64(NumericError::above_upper_limit(
                &projected, &lower, &upper,
            )));
        }

        self.delta = accumulated;
        Ok(&self.delta)
    }

    fn apply_delta(&mut self) -> &Self {
        let delta = self.delta;

        self.value = self.value + delta;
        self.delta = 0;
        self
    }

    fn limits(&self) -> Option<(&i64, &i64)> {
        let (lower, upper) = &self.limits;
        Some((lower, upper))
    }

    fn is_numeric(&self) -> bool {
        true
    }

    fn is_commutative(&self) -> bool {
        true
    }
}

impl CacheableCrdt<i64, i64> for I64 {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_validates_bounds_before_initial_value() {
        assert!(I64::new(-10, 10).is_ok());
        assert!(matches!(
            I64::new(10, -10),
            Err(StateError::I64(NumericError::InvalidLimits(_)))
        ));
    }
}
