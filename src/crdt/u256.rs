use super::state::Numeric;
use super::state::Value;
use alloy_primitives::U256 as AlloyU256;
use std::borrow::Cow;

use super::crdt::{CacheableCrdt, Crdt};
use super::state::Error;

#[derive(Clone, PartialEq)]
pub struct U256 {
    pub(crate) value: AlloyU256,
    pub(crate) delta: AlloyU256,
    pub(crate) limits: (AlloyU256, AlloyU256),
}

impl Default for U256 {
    fn default() -> Self {
        Self {
            value: AlloyU256::ZERO,
            delta: AlloyU256::ZERO,
            limits: (AlloyU256::ZERO, AlloyU256::MAX),
        }
    }
}

impl From<U256> for Value<'static> {
    fn from(value: U256) -> Self {
        Value::Numeric(Numeric::U256(Cow::Owned(value)))
    }
}

impl U256 {
    pub fn new(upper: AlloyU256, lower: AlloyU256) -> Result<Self, Error> {
        Self::check_limits(upper, lower, AlloyU256::ZERO)?;
        Ok(Self {
            value: AlloyU256::ZERO,
            delta: AlloyU256::ZERO,
            limits: (AlloyU256::from(lower), AlloyU256::from(upper)),
        })
    }

    fn check_limits(upper: AlloyU256, lower: AlloyU256, value: AlloyU256) -> Result<(), Error> {
        if lower > upper {
            return Err(Error::U256("lower limit must be less than upper limit"));
        }

        if value < lower {
            return Err(Error::U256("value is below the configured lower limit"));
        }

        if value > upper {
            return Err(Error::U256("value is above the configured upper limit"));
        }
        Ok(())
    }
}

impl Crdt<AlloyU256, AlloyU256> for U256 {
    type Error = Error;

    fn value(&self) -> Option<&AlloyU256> {
        Some(&self.value)
    }

    fn delta(&self) -> Option<&AlloyU256> {
        Some(&self.delta)
    }

    fn add_delta(&mut self, delta: &AlloyU256) -> Result<&AlloyU256, Self::Error> {
        let accumulated = self
            .delta
            .checked_add(*delta)
            .ok_or(Error::U256("U256 overflow"))?;

        let projected = self
            .value
            .checked_add(accumulated)
            .ok_or(Error::U256("U256 overflow"))?;

        let (lower, upper) = self.limits;
        if projected < lower {
            return Err(Error::U256("value is below the configured lower limit"));
        }

        if projected > upper {
            return Err(Error::U256("value is above the configured upper limit"));
        }

        self.delta = accumulated;
        Ok(&self.delta)
    }

    fn apply_delta(&mut self) -> &Self {
        let delta = self.delta;

        self.value = self.value + delta;
        self.delta = AlloyU256::ZERO;
        self
    }

    fn limits(&self) -> Option<(&AlloyU256, &AlloyU256)> {
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

impl CacheableCrdt<AlloyU256, AlloyU256> for U256 {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
