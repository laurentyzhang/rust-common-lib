use super::{Delta, StateError, Value};
use crate::store::traits::StoreError;

pub struct Tracked<'a> {
    pub(crate) value: Value<'a>,
    pub(crate) reads: u32,
    pub(crate) checks: u32, // Number of times the value has been checked for existence
    pub(crate) writes: u32,
    pub(crate) deltas: u32,
    pub(crate) creates: u32, // Indicates if the tracked value is newly created and not yet committed
    pub(crate) is_new: bool,
    pub(crate) tombstone: bool,
}

impl<'a> Tracked<'a> {
    pub fn owned_clone(&self) -> Tracked<'static> {
        Tracked {
            value: self.value.clone().into_owned(),
            reads: self.reads,
            checks: self.checks,
            writes: self.writes,
            deltas: self.deltas,
            tombstone: self.tombstone,
            creates: self.creates,
            is_new: self.is_new,
        }
    }

    pub fn into_owned(self) -> Tracked<'static> {
        Tracked {
            value: self.value.into_owned(),
            reads: self.reads,
            checks: self.checks,
            writes: self.writes,
            deltas: self.deltas,
            tombstone: self.tombstone,
            creates: self.creates,
            is_new: self.is_new,
        }
    }

    pub fn new_empty() -> Self {
        Self {
            value: Value::None,
            reads: 0,
            checks: 0,
            writes: 0,
            deltas: 0,
            creates: 0,
            is_new: true,
            tombstone: false,
        }
    }

    pub fn new_owned(value: Value<'static>) -> Self {
        Self {
            value,
            reads: 0,
            checks: 0,
            writes: 0,
            deltas: 0,
            creates: 1,
            is_new: true,
            tombstone: false,
        }
    }

    pub fn new_borrowed(value: Value<'a>) -> Self {
        Self {
            value,
            reads: 0,
            checks: 0,
            writes: 0, // Start with 1 write since we are borrowing an existing value
            deltas: 0,
            creates: 0,
            is_new: false,
            tombstone: false,
        }
    }

    /// Borrow the underlying value without recording a read.
    pub fn value(&self) -> &Value<'a> {
        &self.value
    }

    /// Replace the value while preserving access history and recording a write.
    pub fn set(&mut self, value: Value<'a>) -> Result<(), StoreError> {
        if matches!(value, Value::None) {
            return self.delete();
        }

        let is_live = self.is_live();
        let was_tombstone = self.tombstone;
        self.value = value;
        self.tombstone = false;

        if !is_live {
            if was_tombstone {
                self.writes += 1;
            } else {
                self.creates += 1;
            }
            return Ok(());
        }
        self.writes += 1;
        Ok(())
    }

    pub fn delete(&mut self) -> Result<(), StoreError> {
        self.writes += 1;
        if !self.is_live() {
            return Err(StoreError::DeleteNonexistingEntry);
        }
        self.tombstone = true;
        Ok(())
    }

    pub fn get(&mut self) -> Option<&Value<'a>> {
        self.reads += 1;
        if self.is_live() {
            Some(&self.value)
        } else {
            None
        }
    }

    pub fn check(&mut self) {
        self.checks += 1;
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
        self.value.apply_delta();
        Some(&self.value)
    }

    pub fn has_delta(&self) -> bool {
        self.deltas > 0
    }

    pub fn is_read_only(&self) -> bool {
        self.writes == 0 && self.deltas == 0 && self.creates == 0 && !self.tombstone
    }

    pub fn is_creation_cancelled(&self) -> bool {
        self.is_new() && self.is_tombstone()
    }

    pub fn is_live(&self) -> bool {
        !self.is_tombstone() && !self.is_none()
    }

    pub fn is_tombstone(&self) -> bool {
        self.tombstone
    }

    pub fn is_none(&self) -> bool {
        matches!(self.value, Value::None)
    }

    pub fn is_new(&self) -> bool {
        self.is_new
    }
}

#[cfg(test)]
mod tests {
    use super::Tracked;
    use crate::crdt::{
        state::{Delta, Numeric, Value},
        uint64::U64,
    };

    #[test]
    fn write_preserves_history_and_clears_tombstone() {
        let mut tracked = Tracked::new_empty();
        let _ = tracked.get();
        tracked.check();
        tracked
            .set(Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(
                U64::default(),
            ))))
            .unwrap();
        assert!(tracked.add_delta(Delta::None).is_ok());
        let deleted_value = tracked.value().clone();
        assert!(tracked.delete().is_ok());
        assert!(tracked.is_tombstone());
        assert!(tracked.value() == &deleted_value);
        let previous_writes = tracked.writes;
        let value = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));

        tracked.set(value.clone()).unwrap();

        assert_eq!(tracked.reads, 1);
        assert_eq!(tracked.checks, 1);
        assert_eq!(tracked.deltas, 1);
        assert_eq!(tracked.writes, previous_writes + 1);
        assert!(!tracked.is_tombstone());
        assert!(tracked.value() == &value);
    }

    #[test]
    fn only_mutations_make_a_record_non_read_only() {
        let mut tracked = Tracked::new_empty();
        assert!(tracked.is_read_only());

        assert!(tracked.get().is_none());
        assert!(tracked.is_read_only());

        tracked.check();
        assert!(tracked.is_read_only());

        tracked.set(U64::default().into()).unwrap();
        assert!(!tracked.is_read_only());
    }
}
