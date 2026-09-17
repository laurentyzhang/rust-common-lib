use crate::crdt::state::{Numeric, Value};
use alloy_primitives::U256 as AlloyU256;
use std::borrow::Cow;

use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::{NumericError, StateError};

#[derive(Clone, PartialEq)]
pub struct U256 {
    pub(crate) value: AlloyU256,
    pub(crate) delta: AlloyU256,
    pub(crate) delta_subtract: bool,
    pub(crate) limits: (AlloyU256, AlloyU256),
}

impl Default for U256 {
    fn default() -> Self {
        Self {
            value: AlloyU256::ZERO,
            delta: AlloyU256::ZERO,
            delta_subtract: false,
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
    /// Queue a subtraction, returning the magnitude of the net pending delta.
    /// The committed value is unchanged until apply_delta.
    pub fn sub_delta(&mut self, delta: &AlloyU256) -> Result<&AlloyU256, StateError> {
        self.update_delta(delta, true)
    }

    fn update_delta(
        &mut self,
        delta: &AlloyU256,
        subtract: bool,
    ) -> Result<&AlloyU256, StateError> {
        let current = if self.delta_subtract {
            self.value.checked_sub(self.delta)
        } else {
            self.value.checked_add(self.delta)
        };
        let current = current.ok_or_else(|| {
            StateError::U256(if self.delta_subtract {
                NumericError::underflow(&self.value, &self.delta, delta)
            } else {
                NumericError::overflow(&self.value, &self.delta, delta)
            })
        })?;
        let projected = if subtract {
            current.checked_sub(*delta)
        } else {
            current.checked_add(*delta)
        }
        .ok_or_else(|| {
            StateError::U256(if subtract {
                NumericError::underflow(&self.value, &self.delta, delta)
            } else {
                NumericError::overflow(&self.value, &self.delta, delta)
            })
        })?;
        Self::check_limits(self.limits.0, self.limits.1, projected)?;

        // Store a normalized signed magnitude without narrowing the unsigned range.
        self.delta_subtract = projected < self.value;
        self.delta = if self.delta_subtract {
            self.value - projected
        } else {
            projected - self.value
        };
        Ok(&self.delta)
    }

    pub fn new(lower: AlloyU256, upper: AlloyU256) -> Result<Self, StateError> {
        Self::check_limits(lower, upper, AlloyU256::ZERO)?;
        Ok(Self {
            value: AlloyU256::ZERO,
            delta: AlloyU256::ZERO,
            delta_subtract: false,
            limits: (AlloyU256::from(lower), AlloyU256::from(upper)),
        })
    }

    fn check_limits(
        lower: AlloyU256,
        upper: AlloyU256,
        value: AlloyU256,
    ) -> Result<(), StateError> {
        if lower > upper {
            return Err(StateError::U256(NumericError::invalid_limits(
                &lower, &upper,
            )));
        }

        if value < lower {
            return Err(StateError::U256(NumericError::below_lower_limit(
                &value, &lower, &upper,
            )));
        }

        if value > upper {
            return Err(StateError::U256(NumericError::above_upper_limit(
                &value, &lower, &upper,
            )));
        }
        Ok(())
    }
}

impl Crdt<AlloyU256, AlloyU256> for U256 {
    type Error = StateError;

    fn value(&self) -> Option<&AlloyU256> {
        Some(&self.value)
    }

    /// Queue an addition, returning the magnitude of the net pending delta.
    fn add_delta(&mut self, delta: &AlloyU256) -> Result<&AlloyU256, Self::Error> {
        self.update_delta(delta, false)
    }

    fn apply_delta(&mut self) -> &Self {
        self.value = if self.delta_subtract {
            self.value - self.delta
        } else {
            self.value + self.delta
        };
        self.delta = AlloyU256::ZERO;
        self.delta_subtract = false;
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
