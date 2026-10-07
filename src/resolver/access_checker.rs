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
                || operations.removes.count() > 0
        }) else {
            return rejected;
        };

        for record in records {
            let result = first
                .data
                .read_conflict_with(&record.data)
                .and_then(|_| first.data.creation_conflict_with(&record.data))
                .and_then(|_| first.data.delta_conflict_with(&record.data))
                .and_then(|_| first.data.existence_conflict_with(&record.data))
                .and_then(|_| first.data.remove_conflict_with(&record.data));

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

        self.creation_conflict(other, "Creation".to_string())?;
        // self.existence_conflict(other, "Reads".to_string())?;
        // self.read_conflict(other, "Reads".to_string())?;
        self.delta_conflict(other, "Reads".to_string())?;
        self.remove_conflict(other, "Delta".to_string())?;
        Ok(())
    }

    pub fn delta_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.operations.deltas.count() == 0 {
            return Ok(());
        }

        if self.is_newly_created() && other.is_newly_created() {
            return self.created_value_conflict(other);
        }

        self.creation_conflict(other, "Creation".to_string())?;
        // self.existence_conflict(other, "Delta".to_string())?;
        self.read_conflict(other, "Delta".to_string())?;
        // self.delta_conflict(other, "Delta".to_string())?;
        self.remove_conflict(other, "Remove".to_string())?;

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
                AccessConflict::ConcurrentDeltaOnNonCommutative(
                    "Deltas conflict detected with Deltas".to_string(),
                ),
            ));
        }

        Ok(())
    }

    pub fn creation_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if !self.is_newly_created() {
            return Ok(());
        }

        // self.creation_conflict(other, "Creation".to_string())?;
        self.existence_conflict(other, "Existence".to_string())?;
        self.read_conflict(other, "Read".to_string())?;
        if !other.is_newly_created() {
            self.delta_conflict_with(other)?;
        }
        self.remove_conflict(other, "Remove".to_string())?;

        self.created_value_conflict(other)
    }

    // Checks for conflicts arising from existence checks on the current record.
    pub fn existence_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.operations.existence_checks.count() == 0 {
            return Ok(());
        }

        self.creation_conflict(other, "Creation".to_string())?;
        // self.existence_conflict(other, "Existence".to_string())?;
        // self.read_conflict(other, "Read".to_string())?;
        // self.delta_conflict(other, "Delta".to_string())?;
        self.remove_conflict(other, "Remove".to_string())?;
        Ok(())
    }

    // Checks for conflicts arising from remove operations on the current record.
    pub fn remove_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.operations.removes.count() == 0 {
            return Ok(());
        }

        self.creation_conflict(other, "Creation".to_string())?;
        self.existence_conflict(other, "Existence".to_string())?;
        self.read_conflict(other, "Read".to_string())?;
        self.delta_conflict(other, "Delta".to_string())?;
        // self.remove_conflict(other, "Remove".to_string())?;
        Ok(())
    }
}

impl Tracked<Status, Status> {
    fn created_value_conflict(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.value.current != other.value.current {
            return Err(Reason::AccessConflict(AccessConflict::CreationConflict(
                "Creation conflict with differing values".to_string(),
            )));
        }

        Ok(())
    }

    // 1. Checks for conflicts arising from creation operations on the current record.
    fn creation_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), Reason> {
        if other.is_newly_created() {
            return Err(Reason::AccessConflict(AccessConflict::ReadConflict(
                format!("creation_conflict conflict detected with: {}", other_op),
            )));
        }
        Ok(())
    }

    // 2. Checks for conflicts arising from existence checks on the current record.
    fn existence_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), Reason> {
        if other.operations.existence_checks.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::ExistenceConflict(
                format!("Existence conflict detected with: {}", other_op),
            )));
        }
        Ok(())
    }

    // 3. Checks for conflicts arising from read operations on the current record.
    fn read_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), Reason> {
        if other.operations.reads.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::ReadConflict(
                format!("Reads conflict detected with: {}", other_op),
            )));
        }
        Ok(())
    }
    // 4. Checks for conflicts arising from delta operations on the current record.
    fn delta_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), Reason> {
        if other.operations.deltas.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::DeltaConflict(
                format!("Delta conflict detected with: {}", other_op),
            )));
        }
        Ok(())
    }

    //5. Checks for conflicts arising from remove operations on the current record.
    fn remove_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), Reason> {
        if other.operations.removes.count() > 0 {
            return Err(Reason::AccessConflict(AccessConflict::RemoveConflict(
                format!("Remove conflict detected with: {}", other_op),
            )));
        }
        Ok(())
    }
}
