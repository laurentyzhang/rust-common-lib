use super::{Delta, Error, Value};

pub struct Tracked<'a> {
    value: Value<'a>,
    reads: u32,
    checks: u32, // Number of times the value has been checked for existence
    writes: u32,
    deltas: u32,
    tombstone: u32,
    creates: u32, // Indicates if the tracked value is newly created and not yet committed
}

impl<'a> Tracked<'a> {
    pub fn new() -> Self {
        Self {
            value: Value::None,
            reads: 0,
            checks: 0,
            writes: 0,
            deltas: 0,
            tombstone: 0,
            creates: 1,
        }
    }

    pub fn new_owned(value: Value<'a>) -> Self {
        Self {
            value,
            reads: 0,
            checks: 0,
            writes: 1,
            deltas: 0,
            tombstone: 0,
            creates: 1,
        }
    }

    pub fn new_borrowed(value: Value<'a>) -> Self {
        Self {
            value,
            reads: 0,
            checks: 0,
            writes: 0, // Start with 1 write since we are borrowing an existing value
            deltas: 0,
            tombstone: 0,
            creates: 0,
        }
    }

    /// Borrow the underlying value without recording a read.
    pub fn value(&self) -> &Value<'a> {
        &self.value
    }

    pub fn create(&mut self, value: Value<'a>) {
        self.write(value);
        self.writes -= 1;
        self.creates += 1;
    }

    /// Replace the value while preserving access history and recording a write.
    pub fn write(&mut self, value: Value<'a>) {
        self.value = value;
        self.tombstone = 0;
        self.writes += 1;
    }

    pub fn check(&mut self) {
        self.checks += 1;
    }

    pub fn read(&mut self) -> Option<&Value<'a>> {
        self.reads += 1;
        if self.is_live() {
            Some(&self.value)
        } else {
            None
        }
    }

    pub fn add_delta(&mut self, delta: Delta) -> Result<(), Error> {
        self.deltas += 1;
        self.value.add_delta(&delta)
    }

    pub fn apply_delta(&mut self) -> Option<&Value<'a>> {
        self.writes += 1;
        self.value.apply_delta();
        Some(&self.value)
    }

    pub fn delete(&mut self) -> Result<(), Error> {
        self.writes += 1;
        if !self.is_live() {
            return Err(Error::EntryNotFound);
        }

        self.tombstone += 1;
        Ok(())
    }

    pub fn is_live(&self) -> bool {
        !self.is_tombstone() && !self.is_none()
    }

    pub fn is_tombstone(&self) -> bool {
        self.tombstone > 0
    }

    pub fn is_none(&self) -> bool {
        matches!(self.value, Value::None)
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
        let mut tracked = Tracked::new();
        let _ = tracked.read();
        tracked.check();
        assert!(tracked.add_delta(Delta::None).is_ok());
        tracked.write(Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(
            U64::default(),
        ))));
        assert!(tracked.delete().is_ok());
        let previous_writes = tracked.writes;
        let value = Value::Numeric(Numeric::U64(std::borrow::Cow::Owned(U64::default())));

        tracked.write(value.clone());

        assert_eq!(tracked.reads, 1);
        assert_eq!(tracked.checks, 1);
        assert_eq!(tracked.deltas, 1);
        assert_eq!(tracked.writes, previous_writes + 1);
        assert!(!tracked.is_tombstone());
        assert!(tracked.value() == &value);
    }
}
