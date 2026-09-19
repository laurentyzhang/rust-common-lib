use crate::crdt::crdt::{CacheableCrdt, Crdt};
use crate::crdt::state::{DeltaOp, Numeric, NumericError, StateError, Value};
use std::borrow::Cow;

#[derive(Clone, PartialEq)]
pub struct U64 {
    pub(crate) value: u64,
    pub(crate) delta: Option<DeltaOp<u64>>,
    pub(crate) limits: (u64, u64),
}

impl Default for U64 {
    fn default() -> Self {
        Self {
            value: 0,
            delta: None,
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
    fn update_delta(&mut self, operation: DeltaOp<u64>) -> Result<&DeltaOp<u64>, StateError> {
        self.delta = Some(self.try_delta(&operation)?);
        Ok(self.delta.as_ref().unwrap())
    }

    fn try_delta(&self, operation: &DeltaOp<u64>) -> Result<DeltaOp<u64>, StateError> {
        let pending = match &self.delta {
            Some(DeltaOp::Add(value)) | Some(DeltaOp::Sub(value)) => *value,
            None => 0,
        };

        let current = match &self.delta {
            Some(DeltaOp::Add(value)) => self.value + value,
            Some(DeltaOp::Sub(value)) => self.value - value,
            None => self.value,
        };

        let projected = match operation {
            DeltaOp::Add(delta) => current.checked_add(*delta).ok_or_else(|| {
                StateError::U64(NumericError::overflow(&self.value, &pending, delta))
            })?,
            DeltaOp::Sub(delta) => current.checked_sub(*delta).ok_or_else(|| {
                StateError::U64(NumericError::underflow(&self.value, &pending, delta))
            })?,
        };

        Self::check_against_limits(self.limits.0, self.limits.1, projected)?;

        Ok(if projected < self.value {
            DeltaOp::Sub(self.value - projected)
        } else {
            DeltaOp::Add(projected - self.value)
        })
    }

    // fn numeric_value(number: u64) -> Value<'static> {
    //     Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64 {
    //         value: number,
    //         ..U64::default()
    //     })))
    // }

    pub fn new(lower: u64, upper: u64) -> Result<Self, StateError> {
        Self::check_against_limits(lower, upper, 0)?;
        Ok(Self {
            value: 0,
            delta: None,
            limits: (lower, upper),
        })
    }

    fn check_against_limits(lower: u64, upper: u64, value: u64) -> Result<(), StateError> {
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

impl Crdt<u64, DeltaOp<u64>> for U64 {
    type Error = StateError;

    fn value(&self) -> Option<&u64> {
        Some(&self.value)
    }

    fn delta(&self) -> Option<&DeltaOp<u64>> {
        self.delta.as_ref()
    }

    /// Queue an addition, returning the magnitude of the net pending delta.
    fn add_delta(&mut self, delta: &DeltaOp<u64>) -> Result<&DeltaOp<u64>, Self::Error> {
        self.update_delta(delta.clone())
    }

    fn apply_delta(&mut self) -> &Self {
        if let Some(delta) = self.delta.take() {
            self.value = match delta {
                DeltaOp::Add(value) => self.value + value,
                DeltaOp::Sub(value) => self.value - value,
            };
        }
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

impl CacheableCrdt<u64, DeltaOp<u64>> for U64 {
    fn cache_weight(&self) -> usize {
        std::mem::size_of::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use crate::crdt::crdt::Crdt;
    use crate::crdt::state::DeltaOp;

    use super::U64;

    #[test]
    fn constructor_uses_lower_then_upper() {
        assert!(U64::new(0, 100).is_ok());
        assert!(U64::new(100, 0).is_err());
    }

    #[test]
    fn delta_uses_add_and_sub_operations() {
        let mut value = U64::default();

        value.add_delta(&DeltaOp::Add(10)).unwrap();
        value.add_delta(&DeltaOp::Sub(3)).unwrap();
        assert_eq!(value.delta, Some(DeltaOp::Add(7)));

        value.apply_delta();
        assert_eq!(value.value, 7);
        assert_eq!(value.delta, None);

        value.add_delta(&DeltaOp::Sub(2)).unwrap();
        assert_eq!(value.delta, Some(DeltaOp::Sub(2)));

        value.apply_delta();
        assert_eq!(value.value, 5);
        assert_eq!(value.delta, None);
    }
}
