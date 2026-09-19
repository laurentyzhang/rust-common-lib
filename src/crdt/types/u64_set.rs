use crate::collections::delta_set::DeltaSet;
use crate::crdt::crdt::Crdt;
use crate::crdt::state::{DeltaOp, StateError, Value};
use std::borrow::Cow;

#[derive(Clone, PartialEq)]
pub struct U64Set {
    pub(crate) entries: DeltaSet<u64>,
    pub(crate) delta: Option<Vec<DeltaOp<u64>>>,
}

impl Default for U64Set {
    fn default() -> Self {
        Self {
            entries: DeltaSet::new(16),
            delta: None,
        }
    }
}

impl From<U64Set> for Value<'static> {
    fn from(value: U64Set) -> Self {
        Value::U64Set(Cow::Owned(value))
    }
}

impl U64Set {
    pub fn new() -> Result<Self, StateError> {
        Ok(Self::default())
    }
}

impl Crdt<DeltaSet<u64>, [DeltaOp<u64>]> for U64Set {
    type Error = StateError;

    fn value(&self) -> Option<&DeltaSet<u64>> {
        Some(&self.entries)
    }

    fn delta(&self) -> Option<&[DeltaOp<u64>]> {
        self.delta.as_deref()
    }

    fn add_delta(&mut self, delta: &[DeltaOp<u64>]) -> Result<&[DeltaOp<u64>], StateError> {
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
                    DeltaOp::Add(value) => Some(*value),
                    DeltaOp::Sub(_) => None,
                })
                .collect::<Vec<_>>();
            let removed = delta
                .iter()
                .filter_map(|operation| match operation {
                    DeltaOp::Sub(value) => Some(*value),
                    DeltaOp::Add(_) => None,
                })
                .collect::<Vec<_>>();
            self.entries.apply_delta(&removed, &added);
            self.entries.commit();
        }
        self
    }

    fn limits(&self) -> Option<(&DeltaSet<u64>, &DeltaSet<u64>)> {
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
    use super::U64Set;
    use crate::crdt::crdt::Crdt;
    use crate::crdt::state::DeltaOp;

    #[test]
    fn default_is_empty() {
        let set = U64Set::default();

        assert_eq!(set.value().unwrap().len(), 0);
        assert_eq!(set.delta.as_ref(), None);
        assert!(!set.is_numeric());
        assert!(set.is_commutative());
    }

    #[test]
    fn add_delta_does_not_change_value_until_applied() {
        let mut set = U64Set::default();
        let delta = vec![DeltaOp::Add(10), DeltaOp::Add(20)];

        set.add_delta(&delta).unwrap();

        assert_eq!(set.value().unwrap().get(&10), None);
        assert_eq!(set.delta.as_ref(), Some(&delta));
    }

    #[test]
    fn apply_delta_adds_and_removes_entries() {
        let mut set = U64Set::default();
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
        let mut set = U64Set::default();
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
}
