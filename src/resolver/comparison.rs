use crate::crdt::state::{Numeric, Value};
use std::cmp::Ordering;

pub(super) struct ComparableNumeric<'a> {
    value: &'a Numeric<'static>,
    tx_id: u64,
}

impl<'a> ComparableNumeric<'a> {
    pub(super) fn new(value: &'a Numeric<'static>, tx_id: u64) -> Self {
        Self { value, tx_id }
    }

    pub(super) fn compare(&self, other: &Self) -> Ordering {
        let ordering = match (self.value, other.value) {
            (Numeric::I64(left), Numeric::I64(right)) => left.compare(right),
            (Numeric::U64(left), Numeric::U64(right)) => left.compare(right),
            (Numeric::U256(left), Numeric::U256(right)) => left.compare(right),
            _ => unreachable!("a key cannot have different numeric types"),
        };

        ordering.then_with(|| self.tx_id.cmp(&other.tx_id))
    }
}

pub(super) struct ComparableNonNumeric<'a> {
    value: &'a Value<'static>,
    tx_id: u64,
}

impl<'a> ComparableNonNumeric<'a> {
    pub(super) fn new(value: &'a Value<'static>, tx_id: u64) -> Self {
        Self { value, tx_id }
    }

    pub(super) fn compare(&self, other: &Self) -> Ordering {
        let ordering = match (self.value, other.value) {
            (Value::Bytes(left), Value::Bytes(right)) => left.compare(right),
            (Value::U64Set(left), Value::U64Set(right)) => left.compare(right),
            _ => unreachable!("a nonnumeric key cannot have different value types"),
        };

        ordering.then_with(|| self.tx_id.cmp(&other.tx_id))
    }
}

#[cfg(test)]
mod tests {
    use super::ComparableNonNumeric;
    use crate::crdt::{
        Crdt,
        bytes::Bytes,
        state::{DeltaOp, Value},
        u64_set::U64Set,
    };
    use std::cmp::Ordering;

    #[test]
    fn compares_bytes_then_transaction_id() {
        let left = Value::from(Bytes::new(vec![b'a']).unwrap());
        let right = Value::from(Bytes::new(vec![b'z']).unwrap());

        assert_eq!(
            ComparableNonNumeric::new(&left, 30).compare(&ComparableNonNumeric::new(&right, 20)),
            Ordering::Less
        );

        assert_eq!(
            ComparableNonNumeric::new(&left, 20).compare(&ComparableNonNumeric::new(&left, 30)),
            Ordering::Less
        );
    }

    #[test]
    fn compares_u64_sets_then_transaction_id() {
        let mut one = U64Set::default();
        one.add_delta(&[DeltaOp::Add(1)]).unwrap();

        let mut two = U64Set::default();
        two.add_delta(&[DeltaOp::Add(1), DeltaOp::Sub(2)]).unwrap();

        let left = Value::from(one);
        let right = Value::from(two);

        assert_eq!(
            ComparableNonNumeric::new(&left, 30).compare(&ComparableNonNumeric::new(&right, 20)),
            Ordering::Less
        );
    }
}
