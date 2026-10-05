use super::{
    reason::{AccessConflict, Reason},
    traits::{Checker, Rejected},
};
use crate::crdt::state::{
    Tracked,
    status::{Status, Tag},
    value::Value,
};
use crate::execution::output::ExecutionOutput;
use std::collections::{BTreeSet, HashMap};

/// Checks access conflicts for both numeric and nonnumeric records.
pub struct AccessChecker<K> {
    entries: HashMap<K, BTreeSet<ExecutionOutput<K, Tracked<Status, Status>>>>,
}

impl<K> AccessChecker<K> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn import(&mut self, transitions: Vec<ExecutionOutput<K, Tracked<Status, Status>>>)
    where
        K: Clone + Eq + std::hash::Hash,
    {
        for transition in transitions {
            self.entries
                .entry(transition.key.clone())
                .or_default()
                .insert(transition);
        }
    }

    pub fn check_conflicts(&mut self) -> Vec<Rejected<K>>
    where
        K: Send + Sync,
    {
        Self::find_rejections(&mut self.entries)
    }
}

impl<K> Checker<K> for AccessChecker<K> {
    type Input = ExecutionOutput<K, Tracked<Status, Status>>;

    fn find_rejections_by_key(records: BTreeSet<Self::Input>) -> Vec<Rejected<K>> {
        let mut rejected = Vec::new();
        let mut records = records.into_iter();

        let Some(first) = records.find(|record| {
            let operations = &record.data.operations;
            operations.reads.count() > 0
                || operations.existence_checks.count() > 0
                || operations.deltas.count() > 0
                || operations.deletes.count() > 0
        }) else {
            return rejected;
        };

        for record in records {
            let result = first
                .data
                .read_conflict_with(&record.data)
                .and_then(|_| first.data.delta_conflict_with(&record.data))
                .and_then(|_| first.data.existence_conflict_with(&record.data))
                .and_then(|_| first.data.delete_conflict_with(&record.data));

            if let Err(reason) = result {
                rejected.push((record, reason));
            }
        }

        rejected
    }
}

impl Tracked<Status, Status> {
    pub fn read_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.operations.reads.count() == 0 {
            return Ok(());
        }

        if other.operations.deltas.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::ReadWrite(
                "Reads conflict detected with deltas".to_string(),
            )));
        }
        if other.operations.deletes.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::ReadDelete(
                "Reads conflict detected with deletes".to_string(),
            )));
        }
        Ok(())
    }

    pub fn delta_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.operations.deltas.count() == 0 {
            return Ok(());
        }

        if other.operations.reads.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::ReadWrite(
                "Deltas conflict detected with reads".to_string(),
            )));
        }

        if other.operations.deletes.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::DeltaDelete(
                "Deltas conflict detected with deletes".to_string(),
            )));
        }

        // Bytes do not support commutative delta writes.
        // Only idempotent delta writes are allowed for Bytes.
        // Without access to the previous state, we can only
        // safely assume that the default state or deleting the value
        // are actually idempotent.
        if matches!(&other.value.current, Status::Value(Value::Bytes(_)))
            && !matches!(
                &self.value.current,
                Status::Tag(Tag::Missing) | Status::Tag(Tag::Default) | Status::Tag(Tag::Deleted)
            )
        {
            return Err(Reason::AccessConflict(
                AccessConflict::ConcurrentDeltaWritesOnNonCommutative(
                    "Deltas conflict detected with Deltas".to_string(),
                ),
            ));
        }

        Ok(())
    }

    pub fn existence_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.operations.existence_checks.count() == 0 {
            return Ok(());
        }

        if other.operations.deletes.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::ReadWrite(
                "Existence check on deleted value".to_string(),
            )));
        }
        Ok(())
    }

    pub fn delete_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.operations.deletes.count() == 0 {
            return Ok(());
        }

        if other.operations.reads.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::ReadDelete(
                "Deleted value conflicts with reads".to_string(),
            )));
        }

        if other.operations.existence_checks.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::ReadWrite(
                "Deleted value conflicts with existence check".to_string(),
            )));
        }

        if other.operations.deltas.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::DeltaDelete(
                "Deleted value conflicts with deltas".to_string(),
            )));
        }
        Ok(())
    }
}
