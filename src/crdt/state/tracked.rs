use super::{
    Delta, Numeric, StateError, Status, Value, numeric::ComparableNumeric, op::Operations,
    value::Values,
};
use crate::store::traits::StoreError;
use std::cmp::Ordering;

pub struct Tracked<T0, T1> {
    pub(crate) id: u64,
    pub(crate) value: Values<T0, T1>,
    pub(crate) operations: Operations,
}

impl<T0, T1> Tracked<T0, T1> {
    pub fn original(&self) -> &T0 {
        &self.value.original
    }

    pub fn current(&self) -> &T1 {
        &self.value.current
    }

    pub fn into_current(self) -> T1 {
        self.value.current
    }

    pub fn clone_with_states<U0, U1>(&self, value: Values<U0, U1>) -> Tracked<U0, U1> {
        Tracked {
            id: self.id,
            value,
            operations: self.operations,
        }
    }

    pub fn map_current<U>(self, map: impl FnOnce(T1) -> U) -> Tracked<T0, U> {
        Tracked {
            id: self.id,
            value: Values {
                original: self.value.original,
                current: map(self.value.current),
            },
            operations: self.operations,
        }
    }
}

impl Tracked<Status, Status> {
    pub fn compare(&self, other: &Self) -> Ordering {
        (
            self.operations.deltas,
            self.operations.existence_checks,
            self.operations.reads,
            self.id,
        )
            .cmp(&(
                other.operations.deltas,
                other.operations.existence_checks,
                other.operations.reads,
                other.id,
            ))
    }
}

impl Tracked<Status, Numeric<'static>> {
    pub fn compare(&self, other: &Self) -> Ordering {
        let left = ComparableNumeric::new(&self.value.current);
        let right = ComparableNumeric::new(&other.value.current);

        left.compare(&right, || self.id.cmp(&other.id))
    }
}

impl<'a> Tracked<Value<'a>, Value<'a>> {
    pub fn new_owned_empty(id: u64) -> Self {
        Self::from_states(
            id,
            Values {
                original: Value::None,
                current: Value::None,
            },
        )
    }

    pub fn new_owned(value: Value<'a>, id: u64) -> Self {
        Self::from_states(
            id,
            Values {
                original: Value::None,
                current: value,
            },
        )
    }

    pub fn new_borrowed(value: &'a Value<'_>, id: u64) -> Self {
        Self::from_states(
            id,
            Values {
                original: Value::from_borrowed(value),
                current: Value::from_borrowed(value),
            },
        )
    }

    pub fn owned_clone(&self) -> Tracked<Value<'static>, Value<'static>> {
        self.clone_with_states(Values {
            original: self.value.original.clone().into_owned(),
            current: self.value.current.clone().into_owned(),
        })
    }

    pub fn into_owned(self) -> Tracked<Value<'static>, Value<'static>> {
        Tracked {
            id: self.id,
            value: Values {
                original: self.value.original.into_owned(),
                current: self.value.current.into_owned(),
            },
            operations: self.operations,
        }
    }

    /// Init the value with a new non-None value.
    pub fn set(&mut self, value: Value<'a>) -> Result<(), StoreError> {
        if matches!(value, Value::None) {
            return Err(StoreError::SetNoneToValue);
        }

        self.operations.deltas.increment();
        if self.is_live() || self.is_deleted() {
            return Err(StoreError::ValueCannotBeRecreated);
        }

        self.value.current = value;
        Ok(())
    }

    pub fn add_delta(&mut self, delta: Delta) -> Result<(), StateError> {
        if !self.is_live() {
            return Err(StateError::CannotAddDeltaToMissingValue);
        }
        self.operations.deltas.increment();
        self.value.current.add_delta(&delta)
    }

    pub fn delete(&mut self) -> Result<(), StoreError> {
        self.operations.deletes.increment();
        self.value.current = Value::None;
        Ok(())
    }

    pub fn get(&mut self) -> Option<&Value<'a>> {
        self.operations.reads.increment();
        self.is_live().then_some(&self.value.current)
    }

    pub fn check(&mut self) {
        self.operations.existence_checks.increment();
    }

    pub fn apply_delta(&mut self) -> Option<&Value<'a>> {
        self.operations.deltas.increment();
        if !self.is_live() {
            return None;
        }
        self.value.current.apply_delta();
        Some(&self.value.current)
    }

    pub fn has_delta(&self) -> bool {
        self.operations.deltas.count() > 0
    }

    pub fn is_creation_cancelled(&self) -> bool {
        matches!(self.value.original, Value::None) && matches!(self.value.current, Value::None)
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.value.current, Value::None)
    }

    pub fn is_missing(&self) -> bool {
        matches!(self.value.current, Value::None)
    }

    pub fn is_deleted(&self) -> bool {
        matches!(self.value.current, Value::None) && !matches!(self.value.original, Value::None)
    }

    pub fn is_created(&self) -> bool {
        matches!(self.value.original, Value::None) && !matches!(self.value.current, Value::None)
    }

    pub fn is_preexisting(&self) -> bool {
        !matches!(self.value.original, Value::None)
    }

    pub fn is_read_only(&self) -> bool {
        self.operations.deltas.count() == 0 && self.operations.deletes.count() == 0
    }
}

impl<'a> Tracked<Value<'a>, Value<'a>> {
    fn from_states(id: u64, value: Values<Value<'a>, Value<'a>>) -> Self {
        Self {
            id,
            value,
            operations: Operations::default(),
        }
    }
}
