use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::{Numeric, NumericError, StateError, Value};
use std::borrow::Cow;

#[derive(Clone, PartialEq)]
pub struct U64 {
    pub(crate) value: u64,
    pub(crate) delta: u64,
    pub(crate) delta_subtract: bool,
    pub(crate) limits: (u64, u64),
}

impl Default for U64 {
    fn default() -> Self {
        Self {
            value: 0,
            delta: 0,
            delta_subtract: false,
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
    /// Queue a subtraction, returning the magnitude of the net pending delta.
    /// The committed value is unchanged until apply_delta.
    pub fn sub_delta(&mut self, delta: &u64) -> Result<&u64, StateError> {
        self.update_delta(delta, true)
    }

    fn update_delta(&mut self, delta: &u64, subtract: bool) -> Result<&u64, StateError> {
        let current = if self.delta_subtract {
            self.value.checked_sub(self.delta)
        } else {
            self.value.checked_add(self.delta)
        };
        let current = current.ok_or_else(|| {
            StateError::U64(if self.delta_subtract {
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
            StateError::U64(if subtract {
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
            delta_subtract: false,
            limits: (lower, upper),
        })
    }

    fn check_limits(lower: u64, upper: u64, value: u64) -> Result<(), StateError> {
        if lower > upper {
            return Err(StateError::U64(NumericError::invalid_limits(
                &lower, &upper,
            )));
        }

        if value < lower {
            return Err(StateError::U64(NumericError::below_lower_limit(
                &value, &lower, &upper,
            )));
        }

        if value > upper {
            return Err(StateError::U64(NumericError::above_upper_limit(
                &value, &lower, &upper,
            )));
        }
        Ok(())
    }
}

impl Crdt<u64, u64> for U64 {
    type Error = StateError;

    fn value(&self) -> Option<&u64> {
        Some(&self.value)
    }

    /// Queue an addition, returning the magnitude of the net pending delta.
    fn add_delta(&mut self, delta: &u64) -> Result<&u64, Self::Error> {
        self.update_delta(delta, false)
    }

    fn apply_delta(&mut self) -> &Self {
        self.value = if self.delta_subtract {
            self.value - self.delta
        } else {
            self.value + self.delta
        };
        self.delta = 0;
        self.delta_subtract = false;
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
