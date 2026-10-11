use crate::collections::delta_set::DeltaSet;
use crate::crdt::crdt::Crdt;
use crate::crdt::state::{DeltaOp, StateError};
use std::cmp::Ordering;
use std::hash::Hash;

#[derive(Clone, Debug, PartialEq)]
pub struct Set<T>
where
    T: Eq + Hash,
{
    pub(crate) entries: DeltaSet<T>,
    pub(crate) delta: Option<Vec<DeltaOp<T>>>,
}

impl<T> Default for Set<T>
where
    T: Eq + Hash,
{
    fn default() -> Self {
        Self {
            entries: DeltaSet::new(16),
            delta: None,
        }
    }
}

impl<T> Set<T>
where
    T: Eq + Hash,
{
    pub fn new() -> Result<Self, StateError> {
        Ok(Self::default())
    }

    pub fn compare(&self, other: &Self) -> Ordering {
        match (&self.delta, &other.delta) {
            (Some(left), Some(right)) => left.len().cmp(&right.len()),
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
        }
    }
}

impl<T> Crdt<DeltaSet<T>, [DeltaOp<T>]> for Set<T>
where
    T: Clone + Eq + Hash,
{
    type Error = StateError;

    fn value(&self) -> Option<&DeltaSet<T>> {
        Some(&self.entries)
    }

    fn delta(&self) -> Option<&[DeltaOp<T>]> {
        self.delta.as_deref()
    }

    fn add_delta(&mut self, delta: &[DeltaOp<T>]) -> Result<&[DeltaOp<T>], StateError> {
        let pending = self.delta.get_or_insert_default();

        for operation in delta {
            if !pending.contains(operation) {
                pending.push(operation.clone());
            }
        }

        Ok(pending.as_slice())
    }

    fn apply_delta(&mut self) -> &Self {
        if let Some(delta) = self.delta.take() {
            let added = delta
                .iter()
                .filter_map(|operation| match operation {
                    DeltaOp::Add(value) => Some(value.clone()),
                    DeltaOp::Sub(_) => None,
                })
                .collect::<Vec<_>>();
            let removed = delta
                .iter()
                .filter_map(|operation| match operation {
                    DeltaOp::Sub(value) => Some(value.clone()),
                    DeltaOp::Add(_) => None,
                })
                .collect::<Vec<_>>();
            self.entries.apply_delta(&removed, &added);
            self.entries.commit();
        }
        self
    }

    fn limits(&self) -> Option<(&DeltaSet<T>, &DeltaSet<T>)> {
        None
    }

    fn is_numeric(&self) -> bool {
        false
    }

    fn is_commutative(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Set;
    use crate::crdt::crdt::Crdt;
    use crate::crdt::state::DeltaOp;
    use std::cmp::Ordering;

    #[test]
    fn default_is_empty() {
        let set = Set::<u64>::default();

        assert_eq!(set.value().unwrap().len(), 0);
        assert_eq!(set.delta.as_ref(), None);
        assert!(!set.is_numeric());
        assert!(set.is_commutative());
    }

    #[test]
    fn add_delta_does_not_change_value_until_applied() {
        let mut set = Set::default();
        let delta = vec![DeltaOp::Add(10), DeltaOp::Add(20)];

        set.add_delta(&delta).unwrap();

        assert_eq!(set.value().unwrap().get(&10), None);
        assert_eq!(set.delta.as_ref(), Some(&delta));
    }

    #[test]
    fn supports_fixed_length_byte_arrays() {
        let mut set = Set::<[u8; 32]>::default();
        let value = [7; 32];

        set.add_delta(&[DeltaOp::Add(value)]).unwrap();
        set.apply_delta();

        assert_eq!(set.value().unwrap().get(&value), Some(&value));
    }

    #[test]
    fn apply_delta_adds_and_removes_entries() {
        let mut set = Set::default();
        set.add_delta(&[DeltaOp::Add(10), DeltaOp::Add(20)])
            .unwrap();
        set.apply_delta();

        set.add_delta(&[DeltaOp::Add(30), DeltaOp::Sub(10)])
            .unwrap();
        set.apply_delta();

        let entries = set.value().unwrap();
        assert_eq!(entries.get(&10), None);
        assert_eq!(entries.get(&20), Some(&20));
        assert_eq!(entries.get(&30), Some(&30));
        assert_eq!(set.delta.as_ref(), None);
    }

    #[test]
    fn add_delta_accumulates_unique_entries() {
        let mut set = Set::default();
        set.add_delta(&[DeltaOp::Add(10), DeltaOp::Add(20), DeltaOp::Sub(30)])
            .unwrap();
        set.add_delta(&[
            DeltaOp::Add(20),
            DeltaOp::Add(30),
            DeltaOp::Sub(30),
            DeltaOp::Sub(40),
        ])
        .unwrap();

        assert_eq!(
            set.delta.as_ref(),
            Some(&vec![
                DeltaOp::Add(10),
                DeltaOp::Add(20),
                DeltaOp::Sub(30),
                DeltaOp::Add(30),
                DeltaOp::Sub(40),
            ])
        );

        set.apply_delta();
        let entries = set.value().unwrap();
        assert_eq!(entries.get(&10), Some(&10));
        assert_eq!(entries.get(&20), Some(&20));
        assert_eq!(entries.get(&30), Some(&30));
        assert_eq!(entries.get(&40), None);
    }

    #[test]
    fn compare_uses_operation_count_and_places_none_last() {
        let mut one_operation = Set::default();
        one_operation.add_delta(&[DeltaOp::Add(1)]).unwrap();

        let mut two_operations = Set::default();
        two_operations
            .add_delta(&[DeltaOp::Add(1), DeltaOp::Sub(2)])
            .unwrap();

        let no_delta = Set::default();

        assert_eq!(one_operation.compare(&two_operations), Ordering::Less);
        assert_eq!(two_operations.compare(&one_operation), Ordering::Greater);
        assert_eq!(one_operation.compare(&one_operation), Ordering::Equal);
        assert_eq!(one_operation.compare(&no_delta), Ordering::Less);
        assert_eq!(no_delta.compare(&one_operation), Ordering::Greater);
        assert_eq!(no_delta.compare(&no_delta), Ordering::Equal);
    }
}
