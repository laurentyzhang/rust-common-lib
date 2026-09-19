use crate::crdt::state::{DeltaOp, Numeric, Value};
use alloy_primitives::U256 as AlloyU256;
use std::borrow::Cow;

use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::{NumericError, StateError};

#[derive(Clone, PartialEq)]
pub struct U256 {
    pub(crate) value: AlloyU256,
    pub(crate) delta: Option<DeltaOp<AlloyU256>>,
    pub(crate) limits: (AlloyU256, AlloyU256),
}

impl Default for U256 {
    fn default() -> Self {
        Self {
            value: AlloyU256::ZERO,
            delta: None,
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
    pub fn new(lower: AlloyU256, upper: AlloyU256) -> Result<Self, StateError> {
        Self::check_against_limits(lower, upper, AlloyU256::ZERO)?;
        Ok(Self {
            value: AlloyU256::ZERO,
            delta: None,
            limits: (AlloyU256::from(lower), AlloyU256::from(upper)),
        })
    }

    fn update_delta(
        &mut self,
        operation: DeltaOp<AlloyU256>,
    ) -> Result<&DeltaOp<AlloyU256>, StateError> {
        self.delta = Some(self.try_delta(&operation)?);
        Ok(self.delta.as_ref().unwrap())
    }

    fn try_delta(&self, operation: &DeltaOp<AlloyU256>) -> Result<DeltaOp<AlloyU256>, StateError> {
        let current = match &self.delta {
            Some(DeltaOp::Add(delta)) => self.value + delta,
            Some(DeltaOp::Sub(delta)) => self.value - delta,
            None => self.value,
        };

        let pending = match &self.delta {
            Some(DeltaOp::Add(delta)) | Some(DeltaOp::Sub(delta)) => delta,
            None => match operation {
                DeltaOp::Add(delta) | DeltaOp::Sub(delta) => delta,
            },
        };

        let projected = match operation {
            DeltaOp::Add(delta) => current.checked_add(*delta).ok_or_else(|| {
                StateError::U256(NumericError::overflow(&self.value, pending, delta))
            })?,
            DeltaOp::Sub(delta) => current.checked_sub(*delta).ok_or_else(|| {
                StateError::U256(NumericError::underflow(&self.value, pending, delta))
            })?,
        };

        Self::check_against_limits(self.limits.0, self.limits.1, projected)?;
        Ok(if projected < self.value {
            DeltaOp::Sub(self.value - projected)
        } else {
            DeltaOp::Add(projected - self.value)
        })
    }

    fn check_against_limits(
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

impl Crdt<AlloyU256, DeltaOp<AlloyU256>> for U256 {
    type Error = StateError;

    fn value(&self) -> Option<&AlloyU256> {
        Some(&self.value)
    }

    fn delta(&self) -> Option<&DeltaOp<AlloyU256>> {
        self.delta.as_ref()
    }

    /// Queue an addition, returning the magnitude of the net pending delta.
    fn add_delta(
        &mut self,
        delta: &DeltaOp<AlloyU256>,
    ) -> Result<&DeltaOp<AlloyU256>, Self::Error> {
        self.update_delta(delta.clone())
    }

    fn apply_delta(&mut self) -> &Self {
        if let Some(operation) = self.delta.take() {
            self.value = match operation {
                DeltaOp::Add(delta) => self.value + delta,
                DeltaOp::Sub(delta) => self.value - delta,
            };
        }
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

impl CacheableCrdt<AlloyU256, DeltaOp<AlloyU256>> for U256 {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::U256;
    use alloy_primitives::U256 as AlloyU256;

    #[test]
    fn constructor_uses_lower_then_upper() {
        assert!(U256::new(AlloyU256::ZERO, AlloyU256::from(100)).is_ok());
        assert!(U256::new(AlloyU256::from(100), AlloyU256::ZERO).is_err());
    }
}
