//! Commutative set CRDT.

use super::SetDelta;
use crate::collections::delta_set::DeltaSet;
use crate::crdt::crdt::Crdt;
use crate::crdt::state::CrdtError;
use std::cmp::Ordering;
use std::hash::Hash;

#[derive(Clone, Debug, PartialEq)]
pub struct Set<T>
where
    T: Eq + Hash,
{
    pub(crate) entries: DeltaSet<T>,
    pub(crate) delta: Option<SetDelta<T>>,
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

    /// Returns the live element count, including pending additions and removals.
    pub fn len(&self) -> u64 {
        let committed = self.entries.len();
        match &self.delta {
            Some(delta) => committed + delta.added.len() - delta.removed.len() as u64,
            None => committed,
        }
    }

    /// Returns the index span of the committed-plus-pending view, including holes.
    pub fn span(&self) -> u64 {
        self.entries.span() + self.delta.as_ref().map_or(0, |delta| delta.added.span())
    }

    /// Reads index `i` in the committed-plus-pending view: committed entries first, then pending
    /// additions. Removed entries and cancelled additions read as holes (`None`).
    pub fn get_at(&self, i: u64) -> Option<&T>
    where
        T: Ord,
    {
        let span = self.entries.span();
        match &self.delta {
            Some(delta) if i < span => self
                .entries
                .get_at(i)
                .filter(|element| !delta.removed.contains(*element)),
            Some(delta) => delta.added.get_at(i - span),
            None => self.entries.get_at(i),
        }
    }

    /// Returns whether `element` is a member of the committed-plus-pending view.
    pub fn contains(&self, element: &T) -> bool
    where
        T: Ord,
    {
        match &self.delta {
            Some(delta) => {
                delta.added.get(element).is_some()
                    || (self.entries.get(element).is_some() && !delta.removed.contains(element))
            }
            None => self.entries.get(element).is_some(),
        }
    }

    /// Orders sets by their pending change count; sets without a delta come last.
    pub fn compare(&self, other: &Self) -> Ordering {
        match (&self.delta, &other.delta) {
            (Some(left), Some(right)) => (left.added.len() + left.removed.len() as u64)
                .cmp(&(right.added.len() + right.removed.len() as u64)),
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
        }
    }
}

impl<T> Crdt<DeltaSet<T>, SetDelta<T>> for Set<T>
where
    T: Clone + std::fmt::Debug + Eq + Hash + Ord,
{
    type Error = CrdtError;

    fn value(&self) -> Option<&DeltaSet<T>> {
        Some(&self.entries)
    }

    fn delta(&self) -> Option<&SetDelta<T>> {
        self.delta.as_ref()
    }

    /// Merges additions (keeping first-insertion order) and removals into the pending delta,
    /// returning whether anything changed. A call that changes nothing does not create a delta.
    /// Adding a committed entry only cancels its pending removal; only new entries are staged for addition.
    /// Removing a pending addition leaves a hole; only committed entries are staged for removal.
    fn add_delta(&mut self, delta: &SetDelta<T>) -> Result<bool, CrdtError> {
        let had_delta = self.delta.is_some();
        let pending = self.delta.get_or_insert_default();
        let mut changed = false;
        for element in delta.added.iter_live() {
            changed |= if self.entries.get(element).is_some() {
                pending.removed.remove(element)
            } else {
                pending.added.insert(element.clone()).is_some()
            };
        }
        for element in &delta.removed {
            changed |= pending.added.remove(element).is_some()
                || (self.entries.get(element).is_some() && pending.removed.insert(element.clone()));
        }
        if !changed && !had_delta {
            self.delta = None;
        }
        Ok(changed)
    }

    /// Applies removals, then additions, to the entries and clears the delta.
    fn apply_delta(&mut self) -> &Self {
        if let Some(delta) = self.delta.take() {
            let removed = delta.removed.into_iter().collect::<Vec<_>>();
            let added = delta.added.iter_live().cloned().collect::<Vec<_>>();
            self.entries.apply_delta(&removed, &added);
            // Drop holes at commit so committed indexes are always contiguous.
            self.entries.compact();
            self.entries.snapshot();
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
