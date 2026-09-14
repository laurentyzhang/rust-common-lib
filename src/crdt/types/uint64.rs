use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::{Numeric, StateError, Value};
use std::borrow::Cow;

#[derive(Clone, PartialEq)]
pub struct U64 {
    pub(crate) value: u64,
    pub(crate) delta: u64,
    pub(crate) limits: (u64, u64),
}

impl Default for U64 {
    fn default() -> Self {
        Self {
            value: 0,
            delta: 0,
            limits: (u64::MIN, u64::MAX),
        }
    }
}

impl From<U64> for Value<'static> {
    fn from(value: U64) -> Self {
        Value::Numeric(Numeric::U64(Cow::Owned(value)))
    }
}

impl U64 {
    // fn numeric_value(number: u64) -> Value<'static> {
    //     Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64 {
    //         value: number,
    //         ..U64::default()
    //     })))
    // }

    pub fn new(lower: u64, upper: u64) -> Result<Self, StateError> {
        Self::check_limits(lower, upper, 0)?;
        Ok(Self {
            value: 0,
            delta: 0,
            limits: (lower, upper),
        })
    }

    fn check_limits(lower: u64, upper: u64, value: u64) -> Result<(), StateError> {
        if lower > upper {
            return Err(StateError::U64("lower limit must be less than upper limit"));
        }

        if value < lower {
            return Err(StateError::U64("value is below the configured lower limit"));
        }

        if value > upper {
            return Err(StateError::U64("value is above the configured upper limit"));
        }
        Ok(())
    }
}

impl Crdt<u64, u64> for U64 {
    type Error = StateError;

    fn value(&self) -> Option<&u64> {
        Some(&self.value)
    }

    fn delta(&self) -> Option<&u64> {
        Some(&self.delta)
    }

    fn add_delta(&mut self, delta: &u64) -> Result<&u64, StateError> {
        let accumulated = self
            .delta
            .checked_add(*delta)
            .ok_or(StateError::U64("u64 overflow"))?;
        let projected = self
            .value
            .checked_add(accumulated)
            .ok_or(StateError::U64("u64 overflow"))?;

        let (_, upper) = self.limits;
        if projected > upper {
            return Err(StateError::U64("value is above the configured upper limit"));
        }

        self.delta = accumulated;
        Ok(&self.delta)
    }

    fn apply_delta(&mut self) -> &Self {
        if self.delta == 0 {
            return self;
        }

        self.value = self.value + self.delta;
        self.delta = 0;
        self
    }

    fn limits(&self) -> Option<(&u64, &u64)> {
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

impl CacheableCrdt<u64, u64> for U64 {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructor_uses_lower_then_upper() {
        assert!(U64::new(0, 100).is_ok());
        assert!(U64::new(100, 0).is_err());
    }
}
