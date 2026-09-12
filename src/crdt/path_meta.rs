use std::path::Path;

use crate::collections::delta_set::DeltaSet;

use super::crdt::Crdt;
use super::state::Error;

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
    pub fn new() -> Result<Self, Error> {
        Ok(Self::default())
    }
}

impl Crdt<DeltaSet<u64>, PathDelta> for PathMeta {
    type Error = Error;

    fn value(&self) -> Option<&DeltaSet<u64>> {
        Some(&self.entries)
    }

    fn delta(&self) -> Option<&PathDelta> {
        self.delta.as_ref()
    }

    fn add_delta(&mut self, delta: &PathDelta) -> Result<&PathDelta, Error> {
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty() {
        let path = PathMeta::default();

        assert_eq!(path.value().unwrap().len(), 0);
        assert_eq!(path.delta(), None);
        assert!(!path.is_numeric());
        assert!(path.is_commutative());
    }

    #[test]
    fn add_delta_does_not_change_value_until_applied() {
        let mut path = PathMeta::default();
        let delta = PathDelta {
            added: vec![10, 20],
            removed: vec![],
        };

        path.add_delta(&delta).unwrap();

        assert_eq!(path.value().unwrap().get(&10), None);
        assert_eq!(path.delta(), Some(&delta));
    }

    #[test]
    fn apply_delta_adds_and_removes_entries() {
        let mut path = PathMeta::default();
        path.add_delta(&PathDelta {
            added: vec![10, 20],
            removed: vec![],
        })
        .unwrap();
        path.apply_delta();

        path.add_delta(&PathDelta {
            added: vec![30],
            removed: vec![10],
        })
        .unwrap();
        path.apply_delta();

        let entries = path.value().unwrap();
        assert_eq!(entries.get(&10), None);
        assert_eq!(entries.get(&20), Some(&20));
        assert_eq!(entries.get(&30), Some(&30));
        assert_eq!(path.delta(), None);
    }

    #[test]
    fn add_delta_accumulates_unique_entries() {
        let mut path = PathMeta::default();
        path.add_delta(&PathDelta {
            added: vec![10, 20],
            removed: vec![30],
        })
        .unwrap();
        path.add_delta(&PathDelta {
            added: vec![20, 30],
            removed: vec![30, 40],
        })
        .unwrap();

        assert_eq!(
            path.delta(),
            Some(&PathDelta {
                added: vec![10, 20, 30],
                removed: vec![30, 40],
            })
        );

        path.apply_delta();
        let entries = path.value().unwrap();
        assert_eq!(entries.get(&10), Some(&10));
        assert_eq!(entries.get(&20), Some(&20));
        assert_eq!(entries.get(&30), Some(&30));
        assert_eq!(entries.get(&40), None);
    }
}
