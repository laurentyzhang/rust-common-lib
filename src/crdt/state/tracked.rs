use super::{Delta, Marker, StateError, Value};
use crate::store::traits::StoreError;

pub struct Tracked<T> {
    pub(crate) id: u64,
    pub(crate) original: T,
    pub(crate) value: T,
    pub(crate) reads: u32,
    pub(crate) existence_checks: u32, // Number of times the value has been checked for existence
    pub(crate) writes: u32,
    pub(crate) deltas: u32,
}

impl<T> Tracked<T> {
    pub fn original(&self) -> &T {
        &self.original
    }

    pub fn value(&self) -> &T {
        &self.value
    }

    pub fn into_value(self) -> T {
        self.value
    }

    pub fn clone_with_values<U>(&self, original: U, value: U) -> Tracked<U> {
        Tracked {
            id: self.id,
            original,
            value,
            reads: self.reads,
            existence_checks: self.existence_checks,
            writes: self.writes,
            deltas: self.deltas,
        }
    }
}

impl<'a> Tracked<Value<'a>> {
    pub fn new_none(id: u64) -> Self {
        Self::from_values(Value::Marker(Marker::None), Value::Marker(Marker::None), id)
    }

    pub fn new_owned_empty(id: u64) -> Self {
        Self::from_values(
            Value::Marker(Marker::Missing),
            Value::Marker(Marker::Missing),
            id,
        )
    }

    pub fn new_owned(value: Value<'static>, id: u64) -> Self {
        Self::from_values(Value::Marker(Marker::Missing), value, id)
    }

    pub fn new_owned_existing(value: Value<'a>, id: u64) -> Self {
        Self::from_values(Value::Marker(Marker::Stripped), value, id)
    }

    pub fn new_owned_deleted(id: u64) -> Self {
        Self::from_values(
            Value::Marker(Marker::Stripped),
            Value::Marker(Marker::Deleted),
            id,
        )
    }

    pub fn new_borrowed(value: &'a Value<'_>, id: u64) -> Self {
        Self::from_values(Value::from_borrowed(value), Value::from_borrowed(value), id)
    }

    fn from_values(original: Value<'a>, value: Value<'a>, id: u64) -> Self {
        Self {
            id,
            original,
            value,
            reads: 0,
            existence_checks: 0,
            writes: 0,
            deltas: 0,
        }
    }

    pub fn owned_clone(&self) -> Tracked<Value<'static>> {
        self.clone_with_values(
            self.original.clone().into_owned(),
            self.value.clone().into_owned(),
        )
    }

    pub fn into_owned(self) -> Tracked<Value<'static>> {
        Tracked {
            id: self.id,
            original: self.original.into_owned(),
            value: self.value.into_owned(),
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
            Value::Marker(Marker::None | Marker::Missing | Marker::Deleted | Marker::Stripped)
        ) {
            return Err(StoreError::ValueCannotBeStripped);
        }

        if self.is_live() || self.is_deleted() {
            self.writes += 1;
            return Err(StoreError::ValueCannotBeRecreated);
        }

        self.value = value;
        Ok(())
    }

    pub fn delete(&mut self) -> Result<(), StoreError> {
        self.writes += 1;
        if !self.is_live() {
            return Err(StoreError::DeleteNonexistingEntry);
        }
        self.value = if self.is_created() {
            Value::Marker(Marker::Missing)
        } else {
            Value::Marker(Marker::Deleted)
        };
        Ok(())
    }

    pub fn get(&mut self) -> Option<&Value<'a>> {
        self.reads += 1;
        self.is_live().then_some(&self.value)
    }

    pub fn check(&mut self) {
        self.existence_checks += 1;
    }

    pub fn add_delta(&mut self, delta: Delta) -> Result<(), StateError> {
        if !self.is_live() {
            return Err(StateError::CannotAddDeltaToMissingValue);
        }
        self.deltas += 1;
        self.value.add_delta(&delta)
    }

    pub fn apply_delta(&mut self) -> Option<&Value<'a>> {
        self.writes += 1;
        if !self.is_live() {
            return None;
        }
        self.value.apply_delta();
        Some(&self.value)
    }

    pub fn has_delta(&self) -> bool {
        self.deltas > 0
    }

    pub fn is_creation_cancelled(&self) -> bool {
        self.is_missing()
            && matches!(self.original, Value::Marker(Marker::Missing))
            && !self.is_read_only()
    }

    pub fn is_live(&self) -> bool {
        !matches!(
            self.value,
            Value::Marker(Marker::None | Marker::Missing | Marker::Deleted | Marker::Stripped)
        )
    }

    pub fn is_deleted(&self) -> bool {
        matches!(self.value, Value::Marker(Marker::Deleted))
    }

    pub fn is_missing(&self) -> bool {
        matches!(self.value, Value::Marker(Marker::Missing))
    }

    pub fn is_created(&self) -> bool {
        matches!(self.original, Value::Marker(Marker::Missing)) && self.is_live()
    }

    pub fn is_preexisting(&self) -> bool {
        !matches!(self.original, Value::Marker(Marker::None | Marker::Missing))
    }

    pub fn is_read_only(&self) -> bool {
        self.writes == 0 && self.deltas == 0 && !self.is_created()
    }
}

#[cfg(test)]
mod tests {
    use super::Tracked;
    use crate::crdt::{
        state::{Delta, Marker, Numeric, Value},
        uint64::U64,
    };

    #[test]
    fn deleting_created_value_returns_it_to_missing() {
        let mut tracked = Tracked::new_owned(U64::default().into(), 7);
        let _ = tracked.get();
        tracked.check();
        assert!(tracked.add_delta(Delta::None).is_ok());
        assert!(tracked.delete().is_ok());
        assert!(tracked.is_missing());
        assert!(matches!(tracked.value(), Value::Marker(Marker::Missing)));

        assert_eq!(tracked.reads, 1);
        assert_eq!(tracked.existence_checks, 1);
        assert_eq!(tracked.deltas, 1);
    }

    #[test]
    fn deleting_preexisting_value_is_permanent() {
        let value: Value<'static> =
            Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));
        let mut tracked = Tracked::new_borrowed(&value, 7);

        assert!(tracked.delete().is_ok());
        assert!(tracked.is_deleted());
        assert_eq!(
            tracked.set(value.clone()),
            Err(crate::store::traits::StoreError::ValueCannotBeRecreated)
        );
    }

    #[test]
    fn constructors_record_id_and_origin() {
        let empty = Tracked::new_owned_empty(10);
        assert!(matches!(empty.original(), Value::Marker(Marker::Missing)));
        assert!(matches!(empty.value(), Value::Marker(Marker::Missing)));

        let none = Tracked::new_none(9);
        assert!(matches!(none.original(), Value::Marker(Marker::None)));
        assert!(matches!(none.value(), Value::Marker(Marker::None)));

        let owned = Tracked::new_owned(U64::default().into(), 11);
        assert_eq!(owned.id, 11);
        assert!(owned.is_created());
        assert!(matches!(owned.original(), Value::Marker(Marker::Missing)));

        let value: Value<'static> = U64::default().into();
        let borrowed = Tracked::new_borrowed(&value, 12);
        assert_eq!(borrowed.id, 12);
        assert!(!borrowed.is_created());
        assert!(borrowed.is_preexisting());
        assert!(borrowed.original() == &value);
        assert!(borrowed.value() == &value);
    }

    #[test]
    fn only_mutations_make_a_record_non_read_only() {
        let value: Value<'static> = U64::default().into();
        let mut tracked = Tracked::new_borrowed(&value, 7);
        assert!(tracked.is_read_only());

        assert!(tracked.get().is_some());
        assert!(tracked.is_read_only());

        tracked.check();
        assert!(tracked.is_read_only());

        assert_eq!(
            tracked.set(U64::default().into()),
            Err(crate::store::traits::StoreError::ValueCannotBeRecreated)
        );
        assert!(!tracked.is_read_only());
    }

    #[test]
    fn ownership_conversions_preserve_id() {
        let value: Value<'static> = U64::default().into();
        let tracked = Tracked::new_borrowed(&value, 42);
        let cloned = tracked.owned_clone();

        assert_eq!(cloned.id, 42);
        assert!(cloned.original() == &value);

        let owned = tracked.into_owned();
        assert_eq!(owned.id, 42);
        assert!(owned.original() == &value);
    }
}
