use super::{Delta, Error, Value};

pub struct Tracked<'a> {
    value: std::borrow::Cow<'a, Value>,
    reads: u32,
    checks: u32, // Number of times the value has been checked for existence
    writes: u32,
    deltas: u32,
    tombstone: bool,
    is_new: bool, // Indicates if the tracked value is newly created and not yet committed
}

impl<'a> Tracked<'a> {
    pub fn new() -> Self {
        Self {
            value: std::borrow::Cow::Owned(Value::None),
            reads: 0,
            checks: 0,
            writes: 0,
            deltas: 0,
            tombstone: false,
            is_new: true,
        }
    }

    pub fn new_owned(value: Value) -> Self {
        Self {
            value: std::borrow::Cow::Owned(value),
            reads: 0,
            checks: 0,
            writes: 1,
            deltas: 0,
            tombstone: false,
            is_new: true,
        }
    }

    pub fn new_borrowed(value: &'a Value) -> Self {
        Self {
            value: std::borrow::Cow::Borrowed(value),
            reads: 0,
            checks: 0,
            writes: 0, // Start with 1 write since we are borrowing an existing value
            deltas: 0,
            tombstone: false,
            is_new: false,
        }
    }

    /// Borrow the underlying value without recording a read.
    pub fn value(&self) -> &Value {
        self.value.as_ref()
    }

    /// Replace the value while preserving access history and recording a write.
    pub fn write(&mut self, value: Value) {
        self.value = std::borrow::Cow::Owned(value);
        self.tombstone = false;
        self.writes += 1;
    }

    pub fn check(&mut self) -> bool {
        self.checks += 1;
        !matches!(self.value.as_ref(), Value::None) && !self.tombstone
    }

    pub fn read(&mut self) -> Option<&Value> {
        self.reads += 1;
        if self.is_live() {
            Some(self.value.as_ref())
        } else {
            None
        }
    }

    pub fn add_delta(&mut self, delta: Delta) -> Result<(), Error> {
        self.deltas += 1;
        self.value.to_mut().add_delta(&delta)
    }

    pub fn apply_delta(&mut self) -> Option<&Value> {
        self.writes += 1;
        self.value.to_mut().apply_delta();
        Some(self.value.as_ref())
    }

    pub fn delete(&mut self) -> Result<(), Error> {
        self.writes += 1;
        if !self.is_live() {
            return Err(Error::EntryNotFound);
        }

        self.tombstone = true;
        Ok(())
    }

    pub fn is_live(&self) -> bool {
        !self.is_tombstone() && !self.is_none()
    }

    pub fn is_tombstone(&self) -> bool {
        self.tombstone
    }

    pub fn is_none(&self) -> bool {
        matches!(self.value.as_ref(), Value::None)
    }
}

#[cfg(test)]
mod tests {
    use super::Tracked;
    use crate::crdt::{
        state::{Delta, Value},
        uint64::U64,
    };

    #[test]
    fn write_preserves_history_and_clears_tombstone() {
        let mut tracked = Tracked::new();
        let _ = tracked.read();
        assert!(!tracked.check());
        assert!(tracked.add_delta(Delta::None).is_ok());
        tracked.write(Value::U64(U64::default()));
        assert!(tracked.delete().is_ok());
        let previous_writes = tracked.writes;
        let value = Value::U64(U64::default());

        tracked.write(value.clone());

        assert_eq!(tracked.reads, 1);
        assert_eq!(tracked.checks, 1);
        assert_eq!(tracked.deltas, 1);
        assert_eq!(tracked.writes, previous_writes + 1);
        assert!(!tracked.is_tombstone());
        assert!(tracked.value() == &value);
    }
}
