use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::Error;
use crate::crdt::state::Numeric;
use crate::crdt::state::Value;
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
    pub fn new(lower: i64, upper: i64) -> Result<Self, Error> {
        Self::check_limits(lower, upper, 0)?;
        Ok(Self {
            value: 0,
            delta: 0,
            limits: (lower, upper),
        })
    }

    fn check_limits(lower: i64, upper: i64, value: i64) -> Result<(), Error> {
        if lower > upper {
            Err(Error::I64("lower limit is above the upper limit"))
        } else if value < lower {
            Err(Error::I64("value is below the configured lower limit"))
        } else if value > upper {
            Err(Error::I64("value is above the configured upper limit"))
        } else {
            Ok(())
        }
    }
}

impl Crdt<i64, i64> for I64 {
    type Error = Error;

    fn value(&self) -> Option<&i64> {
        Some(&self.value)
    }

    fn delta(&self) -> Option<&i64> {
        Some(&self.delta)
    }

    fn add_delta(&mut self, delta: &i64) -> Result<&i64, Self::Error> {
        let accumulated = self.delta.checked_add(*delta).ok_or_else(|| {
            if *delta < 0 {
                Error::I64("i64 underflow")
            } else {
                Error::I64("i64 overflow")
            }
        })?;
        let projected = self.value.checked_add(accumulated).ok_or_else(|| {
            if accumulated < 0 {
                Error::I64("i64 underflow")
            } else {
                Error::I64("i64 overflow")
            }
        })?;

        let (lower, upper) = self.limits;
        if projected < lower {
            return Err(Error::I64("value is below the configured lower limit"));
        }
        if projected > upper {
            return Err(Error::I64("value is above the configured upper limit"));
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
            Err(Error::I64("lower limit is above the upper limit"))
        ));
    }
}
