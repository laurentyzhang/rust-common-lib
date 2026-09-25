use super::{
    Delta, Marker, StateError, Value,
    op::{Counter, Delta as DeltaOperation, ExistenceCheck, Read, Write},
};
use crate::store::traits::StoreError;

pub struct Tracked<T> {
    pub(crate) id: u64,
    pub(crate) original: T,
    pub(crate) current: T,
    pub(crate) reads: Counter<Read>,
    pub(crate) existence_checks: Counter<ExistenceCheck>,
    pub(crate) writes: Counter<Write>,
    pub(crate) deltas: Counter<DeltaOperation>,
}

impl<T> Tracked<T> {
    pub fn original(&self) -> &T {
        &self.original
    }

    pub fn current(&self) -> &T {
        &self.current
    }

    pub fn into_current(self) -> T {
        self.current
    }

    pub fn clone_with_states<U>(&self, original: U, current: U) -> Tracked<U> {
        Tracked {
            id: self.id,
            original,
            current,
            reads: self.reads,
            existence_checks: self.existence_checks,
            writes: self.writes,
            deltas: self.deltas,
        }
    }
}

impl<'a> Tracked<Value<'a>> {
    pub fn new_owned_empty(id: u64) -> Self {
        Self::from_states(
            Value::Marker(Marker::Missing),
            Value::Marker(Marker::Missing),
            id,
        )
    }

    pub fn new_owned(value: Value<'static>, id: u64) -> Self {
        Self::from_states(Value::Marker(Marker::Missing), value, id)
    }

    pub fn new_owned_existing(value: Value<'a>, id: u64) -> Self {
        Self::from_states(Value::Marker(Marker::Stripped), value, id)
    }

    pub fn new_owned_deleted(id: u64) -> Self {
        Self::from_states(
            Value::Marker(Marker::Stripped),
            Value::Marker(Marker::Deleted),
            id,
        )
    }

    pub fn new_borrowed(value: &'a Value<'_>, id: u64) -> Self {
        Self::from_states(Value::from_borrowed(value), Value::from_borrowed(value), id)
    }

    pub fn owned_clone(&self) -> Tracked<Value<'static>> {
        self.clone_with_states(
            self.original.clone().into_owned(),
            self.current.clone().into_owned(),
        )
    }

    pub fn into_owned(self) -> Tracked<Value<'static>> {
        Tracked {
            id: self.id,
            original: self.original.into_owned(),
            current: self.current.into_owned(),
            reads: self.reads,
            existence_checks: self.existence_checks,
            writes: self.writes,
            deltas: self.deltas,
        }
    }

    /// Replace the value while preserving access history and recording a write.
    pub fn set(&mut self, value: Value<'a>) -> Result<(), StoreError> {
        if matches!(
            value,
            Value::Marker(Marker::Missing | Marker::Deleted | Marker::Stripped)
        ) {
            return Err(StoreError::ValueCannotBeStripped);
        }

        if self.is_live() || self.is_deleted() {
            self.writes.increment();
            return Err(StoreError::ValueCannotBeRecreated);
        }

        self.current = value;
        Ok(())
    }

    pub fn delete(&mut self) -> Result<(), StoreError> {
        self.writes.increment();
        if !self.is_live() {
            return Err(StoreError::DeleteNonexistingEntry);
        }
        self.current = if self.is_created() {
            Value::Marker(Marker::Missing)
        } else {
            Value::Marker(Marker::Deleted)
        };
        Ok(())
    }

    pub fn get(&mut self) -> Option<&Value<'a>> {
        self.reads.increment();
        self.is_live().then_some(&self.current)
    }

    pub fn check(&mut self) {
        self.existence_checks.increment();
    }

    pub fn add_delta(&mut self, delta: Delta) -> Result<(), StateError> {
        if !self.is_live() {
            return Err(StateError::CannotAddDeltaToMissingValue);
        }
        self.deltas.increment();
        self.current.add_delta(&delta)
    }

    pub fn apply_delta(&mut self) -> Option<&Value<'a>> {
        self.writes.increment();
        if !self.is_live() {
            return None;
        }
        self.current.apply_delta();
        Some(&self.current)
    }

    pub fn has_delta(&self) -> bool {
        self.deltas.count() > 0
    }

    pub fn is_creation_cancelled(&self) -> bool {
        matches!(self.original, Value::Marker(Marker::Missing))
            && matches!(self.current, Value::Marker(Marker::Missing))
    }

    pub fn is_live(&self) -> bool {
        !matches!(
            self.current,
            Value::Marker(Marker::Missing | Marker::Deleted)
        )
    }

    pub fn is_missing(&self) -> bool {
        matches!(self.current, Value::Marker(Marker::Missing))
    }

    pub fn is_deleted(&self) -> bool {
        matches!(self.current, Value::Marker(Marker::Deleted))
    }

    pub fn is_created(&self) -> bool {
        matches!(self.original, Value::Marker(Marker::Missing))
            && !matches!(self.current, Value::Marker(Marker::Missing))
    }

    pub fn is_preexisting(&self) -> bool {
        !matches!(self.original, Value::Marker(Marker::Missing))
    }

    pub fn is_read_only(&self) -> bool {
        self.writes.count() == 0 && self.deltas.count() == 0 && !self.is_created()
    }

    pub fn has_conflict_with(&self, other: Tracked<Value<'a>>) -> bool {
        if self.is_missing() || other.is_missing() {
            return false;
        }
        true
    }
}

//
impl<'a> Tracked<Value<'a>> {
    fn from_states(original: Value<'a>, current: Value<'a>, id: u64) -> Self {
        Self {
            id,
            original,
            current,
            reads: Counter::default(),
            existence_checks: Counter::default(),
            writes: Counter::default(),
            deltas: Counter::default(),
        }
    }
}
