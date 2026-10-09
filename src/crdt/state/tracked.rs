use super::{
    Delta, Numeric, StateError, Status, CrdtValue, numeric::ComparableNumeric, op::Operations,
    status::Tag, value::Values,
};

// use crate::crdt::state::status::Tag;
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
    pub fn is_newly_created(&self) -> bool {
        return matches!(&self.value.original, Status::Tag(Tag::Missing))
            && !matches!(&self.value.current, Status::Tag(Tag::Missing));
    }

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

impl<'a> Tracked<CrdtValue<'a>, CrdtValue<'a>> {
    pub fn new_owned_empty(id: u64) -> Self {
        Self::from_states(
            id,
            Values {
                original: CrdtValue::None,
                current: CrdtValue::None,
            },
        )
    }

    pub fn new_owned(value: CrdtValue<'a>, id: u64) -> Self {
        Self::from_states(
            id,
            Values {
                original: CrdtValue::None,
                current: value,
            },
        )
    }

    pub fn new_owned_existing(value: CrdtValue<'a>, id: u64) -> Self {
        Self::from_states(
            id,
            Values {
                original: value.clone(),
                current: value,
            },
        )
    }

    pub fn new_borrowed(value: &'a CrdtValue<'_>, id: u64) -> Self {
        Self::from_states(
            id,
            Values {
                original: CrdtValue::from_borrowed(value),
                current: CrdtValue::from_borrowed(value),
            },
        )
    }

    pub fn owned_clone(&self) -> Tracked<CrdtValue<'static>, CrdtValue<'static>> {
        self.clone_with_states(Values {
            original: self.value.original.clone().into_owned(),
            current: self.value.current.clone().into_owned(),
        })
    }

    pub fn into_owned(self) -> Tracked<CrdtValue<'static>, CrdtValue<'static>> {
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
    pub fn set(&mut self, value: CrdtValue<'a>) -> Result<(), StoreError> {
        if matches!(value, CrdtValue::None) {
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

    pub fn remove(&mut self) -> Result<(), StoreError> {
        self.operations.removes.increment();
        self.value.current = CrdtValue::None;
        Ok(())
    }

    pub fn get(&mut self) -> Option<&CrdtValue<'a>> {
        self.operations.reads.increment();
        self.is_live().then_some(&self.value.current)
    }

    pub fn check(&mut self) {
        self.operations.existence_checks.increment();
    }

    pub fn apply_delta(&mut self) -> Option<&CrdtValue<'a>> {
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
        matches!(self.value.original, CrdtValue::None) && matches!(self.value.current, CrdtValue::None)
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.value.current, CrdtValue::None)
    }

    pub fn is_missing(&self) -> bool {
        matches!(self.value.current, CrdtValue::None)
    }

    pub fn is_deleted(&self) -> bool {
        matches!(self.value.current, CrdtValue::None) && !matches!(self.value.original, CrdtValue::None)
    }

    pub fn is_created(&self) -> bool {
        matches!(self.value.original, CrdtValue::None) && !matches!(self.value.current, CrdtValue::None)
    }

    pub fn is_preexisting(&self) -> bool {
        !matches!(self.value.original, CrdtValue::None)
    }

    pub fn is_read_only(&self) -> bool {
        self.operations.deltas.count() == 0 && self.operations.removes.count() == 0
    }
}

impl<'a> Tracked<CrdtValue<'a>, CrdtValue<'a>> {
    fn from_states(id: u64, value: Values<CrdtValue<'a>, CrdtValue<'a>>) -> Self {
        Self {
            id,
            value,
            operations: Operations::default(),
        }
    }
}
