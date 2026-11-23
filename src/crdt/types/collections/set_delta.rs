//! Pending changes of the set CRDT.

use crate::collections::delta_set::DeltaSet;
use std::collections::BTreeSet;
use std::hash::Hash;

/// Pending changes to a set: additions in insertion order (cancelled ones leave holes) and removals in sorted order.
#[derive(Clone, Debug, PartialEq)]
pub struct SetDelta<T>
where
    T: Eq + Hash,
{
    pub(crate) added: DeltaSet<T>,
    pub(crate) removed: BTreeSet<T>,
}

impl<T> Default for SetDelta<T>
where
    T: Eq + Hash,
{
    /// Creates an empty delta.
    fn default() -> Self {
        Self {
            added: DeltaSet::new(0),
            removed: BTreeSet::new(),
        }
    }
}
