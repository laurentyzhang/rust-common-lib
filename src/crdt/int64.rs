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
    pub fn new(upper: i64, lower: i64) -> Result<Self, Error> {
        Self::check_limits(upper, lower, 0)?;
        Ok(Self {
            value: 0,
            delta: 0,
            limits: (lower, upper),
        })
    }

    fn checked_add(left: i64, right: i64) -> Result<i64, Error> {
        left.checked_add(right).ok_or(if right < 0 {
            Error::I64("i64 underflow")
        } else {
            Error::I64("i64 overflow")
        })
    }

    fn check_limits(upper: i64, lower: i64, value: i64) -> Result<(), Error> {
        if value < lower {
            Err(Error::I64("value is below the configured lower limit"))
        } else if value > upper {
            Err(Error::I64("value is above the configured upper limit"))
        } else if upper < lower {
            Err(Error::I64("upper limit is below the lower limit"))
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
        let old_delta = self.delta;
        let accumulated = Self::checked_add(old_delta, *delta)?;
        let projected = Self::checked_add(self.value, accumulated)?;

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
