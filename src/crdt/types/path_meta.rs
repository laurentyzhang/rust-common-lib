use crate::collections::delta_set::DeltaSet;

use crate::crdt::crdt::Crdt;
use crate::crdt::state::StateError;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PathDelta {
    pub added: Vec<u64>,
    pub removed: Vec<u64>,
}

#[derive(Clone, PartialEq)]
pub struct PathMeta {
    pub(crate) entries: DeltaSet<u64>,
    pub(crate) delta: Option<PathDelta>,
}

impl Default for PathMeta {
    fn default() -> Self {
        Self {
            entries: DeltaSet::new(16),
            delta: None,
        }
    }
}

impl PathMeta {
    pub fn new() -> Result<Self, StateError> {
        Ok(Self::default())
    }
}

impl Crdt<DeltaSet<u64>, PathDelta> for PathMeta {
    type Error = StateError;

    fn value(&self) -> Option<&DeltaSet<u64>> {
        Some(&self.entries)
    }

    fn add_delta(&mut self, delta: &PathDelta) -> Result<&PathDelta, StateError> {
        let pending = self.delta.get_or_insert_with(PathDelta::default);

        for key in &delta.added {
            if !pending.added.contains(key) {
                pending.added.push(*key);
            }
        }
        for key in &delta.removed {
            if !pending.removed.contains(key) {
                pending.removed.push(*key);
            }
        }

        Ok(pending)
    }

    fn apply_delta(&mut self) -> &Self {
        if let Some(delta) = self.delta.take() {
            self.entries.apply_delta(&delta.removed, &delta.added);
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
