mod i64;
mod traits;
mod u256;
mod u64;

pub use traits::Type;

use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::{DeltaOp, NumericError, StateError};
use std::cmp::Ordering;

#[derive(Clone, Debug, PartialEq)]
pub struct Scalar<T: Type> {
    pub(crate) value: T,
    pub(crate) delta: Option<DeltaOp<T::Magnitude>>,
    pub(crate) limits: (T, T),
}

impl<T: Type> Default for Scalar<T> {
    fn default() -> Self {
        Self {
            value: T::ZERO,
            delta: None,
            limits: (T::MIN, T::MAX),
        }
    }
}

impl<T: Type> Scalar<T> {
    pub fn new(lower: T, upper: T) -> Result<Self, StateError> {
        Self::check_against_limits(lower, upper, T::ZERO)?;
        Ok(Self {
            value: T::ZERO,
            delta: None,
            limits: (lower, upper),
        })
    }

    pub fn from_delta(delta: DeltaOp<T::Magnitude>) -> Self {
        Self {
            delta: Some(delta),
            ..Self::default()
        }
    }

    pub fn compare(&self, other: &Self) -> Ordering {
        let zero = T::Magnitude::default();
        match (&self.delta, &other.delta) {
            (Some(DeltaOp::Sub(left)), Some(DeltaOp::Sub(right))) => right.cmp(left),
            (Some(DeltaOp::Sub(left)), _) if *left != zero => Ordering::Less,
            (_, Some(DeltaOp::Sub(right))) if *right != zero => Ordering::Greater,
            (Some(DeltaOp::Add(left)), Some(DeltaOp::Add(right))) => left.cmp(right),
            (Some(DeltaOp::Add(left)), _) => left.cmp(&zero),
            (_, Some(DeltaOp::Add(right))) => zero.cmp(right),
            _ => Ordering::Equal,
        }
    }

    fn update_delta(
        &mut self,
        operation: DeltaOp<T::Magnitude>,
    ) -> Result<&DeltaOp<T::Magnitude>, StateError> {
        self.delta = Some(self.try_delta(&operation)?);
        Ok(self.delta.as_ref().unwrap())
    }

    fn try_delta(
        &self,
        operation: &DeltaOp<T::Magnitude>,
    ) -> Result<DeltaOp<T::Magnitude>, StateError> {
        let pending = match self.delta.as_ref() {
            Some(DeltaOp::Add(delta) | DeltaOp::Sub(delta)) => *delta,
            None => T::Magnitude::default(),
        };

        let current = match self.delta.as_ref() {
            Some(existing) => T::checked_apply(self.value, existing)
                .ok_or_else(|| Self::arithmetic_error(&self.value, &pending, existing))?,
            None => self.value,
        };

        let projected = T::checked_apply(current, operation)
            .ok_or_else(|| Self::arithmetic_error(&self.value, &pending, operation))?;

        Self::check_against_limits(self.limits.0, self.limits.1, projected)?;
        Ok(T::delta_between(self.value, projected))
    }

    fn arithmetic_error(
        value: &T,
        pending: &T::Magnitude,
        operation: &DeltaOp<T::Magnitude>,
    ) -> StateError {
        let error = match operation {
            DeltaOp::Add(delta) => NumericError::overflow(value, pending, delta),
            DeltaOp::Sub(delta) => NumericError::underflow(value, pending, delta),
        };
        T::state_error(error)
    }

    fn check_against_limits(lower: T, upper: T, value: T) -> Result<(), StateError> {
        if lower > upper {
            return Err(T::state_error(NumericError::invalid_limits(&lower, &upper)));
        }
        if value < lower {
            return Err(T::state_error(NumericError::below_lower_limit(
                &value, &lower, &upper,
            )));
        }
        if value > upper {
            return Err(T::state_error(NumericError::above_upper_limit(
                &value, &lower, &upper,
            )));
        }
        Ok(())
    }
}

impl<T: Type> Crdt<T, DeltaOp<T::Magnitude>> for Scalar<T> {
    type Error = StateError;

    fn value(&self) -> Option<&T> {
        Some(&self.value)
    }

    fn delta(&self) -> Option<&DeltaOp<T::Magnitude>> {
        self.delta.as_ref()
    }

    fn add_delta(
        &mut self,
        delta: &DeltaOp<T::Magnitude>,
    ) -> Result<&DeltaOp<T::Magnitude>, Self::Error> {
        self.update_delta(delta.clone())
    }

    fn apply_delta(&mut self) -> &Self {
        if let Some(operation) = self.delta.take() {
            self.value = T::checked_apply(self.value, &operation)
                .expect("a validated scalar delta must remain applicable");
        }
        self
    }

    fn limits(&self) -> Option<(&T, &T)> {
        Some((&self.limits.0, &self.limits.1))
    }

    fn is_numeric(&self) -> bool {
        true
    }

    fn is_commutative(&self) -> bool {
        true
    }
}

impl<T: Type> CacheableCrdt<T, DeltaOp<T::Magnitude>> for Scalar<T> {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}
