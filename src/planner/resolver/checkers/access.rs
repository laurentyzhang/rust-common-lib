use super::util::run_parallel;
use crate::crdt::state::{
    Tracked,
    status::{Status, Tag},
    value::CrdtValue,
};
use crate::execution::output::ExecutionOutput;
use crate::planner::resolver::{AccessConflict, Diagnostic, Reason};
use crate::planner::scheduler::ID;
use std::collections::{BTreeSet, HashMap};

/// Checks access conflicts for both numeric and nonnumeric records.
pub struct AccessChecker<K> {
    pub(in crate::planner::resolver) entries: BTreeSet<ExecutionOutput<K, Tracked<Status, Status>>>,
    counts: HashMap<K, usize>,
}

impl<K> AccessChecker<K> {
    pub fn new() -> Self {
        Self {
            entries: BTreeSet::new(),
            counts: HashMap::new(),
        }
    }

    pub fn import(&mut self, transitions: Vec<ExecutionOutput<K, Tracked<Status, Status>>>)
    where
        K: Clone + Ord + std::hash::Hash,
    {
        for transition in transitions {
            *self.counts.entry(transition.key.clone()).or_default() += 1;
            self.entries.insert(transition);
        }
    }

    pub fn check(&mut self) -> (Vec<Diagnostic<K>>, BTreeSet<(ID, ID)>)
    where
        K: Clone + Ord + std::hash::Hash + Send + Sync,
    {
        run_parallel(&mut self.entries, &mut self.counts, Self::do_check)
    }

    pub fn do_check(
        mut records: Vec<ExecutionOutput<K, Tracked<Status, Status>>>,
    ) -> (Vec<Diagnostic<K>>, BTreeSet<(ID, ID)>)
    where
        K: Clone + Ord,
    {
        let mut diagnostics = Vec::new();
        let mut conflicts = BTreeSet::new();
        records.retain(|record| {
            let operations = &record.data.operations;
            operations.reads.count() > 0
                || operations.existence_checks.count() > 0
                || operations.deltas.count() > 0
                || operations.removes.count() > 0
        });

        // Report every conflicting pair so the resolver can select either sequence.
        while let Some(record) = records.pop() {
            for first in &records {
                if (record.id.block, record.id.generation, record.id.sequence)
                    == (first.id.block, first.id.generation, first.id.sequence)
                {
                    continue;
                }
                let result = if record.data.operations.reads.count() > 0
                    && record.data.operations.deltas.count() > 0
                    && first.data.operations.deltas.count() > 0
                {
                    Err(Reason::AccessConflict(AccessConflict::ReadConflict(
                        "Read conflict detected with earlier delta".to_string(),
                    )))
                } else {
                    record
                        .data
                        .read_conflict_with(&first.data)
                        .and_then(|_| record.data.creation_conflict_with(&first.data))
                        .and_then(|_| record.data.delta_conflict_with(&first.data))
                        .and_then(|_| record.data.existence_conflict_with(&first.data))
                        .and_then(|_| record.data.remove_conflict_with(&first.data))
                };

                if let Err(reason) = result {
                    conflicts.insert((first.id.min(record.id), first.id.max(record.id)));
                    // Either endpoint may be rejected by selection.
                    diagnostics.push((
                        record.id,
                        record.key.clone(),
                        vec![first.id],
                        reason.clone(),
                    ));
                    diagnostics.push((first.id, record.key.clone(), vec![record.id], reason));
                }
            }
        }
        (diagnostics, conflicts)
    }
}

impl Tracked<Status, Status> {
    pub fn read_conflict_with(&self, other: &Tracked<Status, Status>) -> Result<(), Reason> {
        if self.operations.reads.count() == 0 {
            return Ok(());
        }

        self.creation_conflict(other, "Creation".to_string())?;
        self.delta_conflict(other, "Reads".to_string())?;
        self.remove_conflict(other, "Reads".to_string())?;
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
        self.read_conflict(other, "Delta".to_string())?;
        self.remove_conflict(other, "Remove".to_string())?;

        // Bytes do not support commutative delta writes.
        // Only idempotent delta writes are allowed for Bytes.
        // Without access to the previous state, we can only
        // safely assume that the default state or deleting the value
        // are actually idempotent.
        if matches!(&other.value.current, Status::Value(CrdtValue::Bytes(_)))
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
