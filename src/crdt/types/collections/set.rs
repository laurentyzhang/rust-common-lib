//! Commutative set CRDT.

use crate::collections::delta_set::DeltaSet;
use crate::crdt::crdt::Crdt;
use crate::crdt::state::{CrdtError, DeltaOp};
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
    pub fn new() -> Result<Self, CrdtError> {
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
    type Error = CrdtError;

    fn value(&self) -> Option<&DeltaSet<T>> {
        Some(&self.entries)
    }

    fn delta(&self) -> Option<&[DeltaOp<T>]> {
        self.delta.as_deref()
    }

    fn add_delta(&mut self, delta: &[DeltaOp<T>]) -> Result<&[DeltaOp<T>], CrdtError> {
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
