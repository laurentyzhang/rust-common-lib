use super::util::run_parallel;
use crate::crdt::state::{Tracked, status::Status};
use crate::execution::output::ExecutionOutput;
use crate::planner::resolver::{AccessConflict, Diagnostic, Reason};
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

    pub fn check(&mut self) -> Vec<Diagnostic<K>>
    where
        K: Clone + Ord + std::hash::Hash + Send + Sync,
    {
        run_parallel(&mut self.entries, &mut self.counts, Self::do_check)
    }

    pub fn do_check(
        mut records: Vec<ExecutionOutput<K, Tracked<Status, Status>>>,
    ) -> Vec<Diagnostic<K>>
    where
        K: Clone + Ord,
    {
        let mut diagnostics = Vec::new();
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
                    Err(AccessConflict::ReadConflict(
                        "Read conflict detected with earlier delta".to_string(),
                    ))
                } else {
                    record
                        .data
                        .read_conflict_with(&first.data)
                        .and_then(|_| record.data.creation_conflict_with(&first.data))
                        .and_then(|_| record.data.delta_conflict_with(&first.data))
                        .and_then(|_| record.data.existence_conflict_with(&first.data))
                        .and_then(|_| record.data.remove_conflict_with(&first.data))
                };

                if let Err(error) = result {
                    // Either endpoint may be rejected by selection.
                    let reason = Reason::AccessConflict(record.key.clone(), error);
                    diagnostics.push(Diagnostic {
                        id: record.id,
                        peers: vec![first.id],
                        reason: reason.clone(),
                    });
                    diagnostics.push(Diagnostic {
                        id: first.id,
                        peers: vec![record.id],
                        reason,
                    });
                }
            }
        }
        diagnostics
    }
}

impl Tracked<Status, Status> {
    pub fn read_conflict_with(
        &self,
        other: &Tracked<Status, Status>,
    ) -> Result<(), AccessConflict> {
        if self.operations.reads.count() == 0 {
            return Ok(());
        }

        self.creation_conflict(other, "Creation".to_string())?;
        self.delta_conflict(other, "Reads".to_string())?;
        self.remove_conflict(other, "Reads".to_string())?;
        Ok(())
    }

    pub fn delta_conflict_with(
        &self,
        other: &Tracked<Status, Status>,
    ) -> Result<(), AccessConflict> {
        if self.operations.deltas.count() == 0 {
            return Ok(());
        }

        if self.is_newly_created() && other.is_newly_created() {
            return self.created_value_conflict(other);
        }

        self.creation_conflict(other, "Creation".to_string())?;
        self.read_conflict(other, "Delta".to_string())?;
        self.remove_conflict(other, "Remove".to_string())?;

        Ok(())
    }

    pub fn creation_conflict_with(
        &self,
        other: &Tracked<Status, Status>,
    ) -> Result<(), AccessConflict> {
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
    pub fn existence_conflict_with(
        &self,
        other: &Tracked<Status, Status>,
    ) -> Result<(), AccessConflict> {
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
    pub fn remove_conflict_with(
        &self,
        other: &Tracked<Status, Status>,
    ) -> Result<(), AccessConflict> {
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
    fn created_value_conflict(
        &self,
        other: &Tracked<Status, Status>,
    ) -> Result<(), AccessConflict> {
        if self.value.current != other.value.current {
            return Err(AccessConflict::CreationConflict(
                "Creation conflict with differing values".to_string(),
            ));
        }

        Ok(())
    }

    // 1. Checks for conflicts arising from creation operations on the current record.
    fn creation_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), AccessConflict> {
        if other.is_newly_created() {
            return Err(AccessConflict::ReadConflict(format!(
                "creation_conflict conflict detected with: {}",
                other_op
            )));
        }
        Ok(())
    }

    // 2. Checks for conflicts arising from existence checks on the current record.
    fn existence_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), AccessConflict> {
        if other.operations.existence_checks.count() > 0 {
            return Err(AccessConflict::ExistenceConflict(format!(
                "Existence conflict detected with: {}",
                other_op
            )));
        }
        Ok(())
    }

    // 3. Checks for conflicts arising from read operations on the current record.
    fn read_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), AccessConflict> {
        if other.operations.reads.count() > 0 {
            return Err(AccessConflict::ReadConflict(format!(
                "Reads conflict detected with: {}",
                other_op
            )));
        }
        Ok(())
    }
    // 4. Checks for conflicts arising from delta operations on the current record.
    fn delta_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), AccessConflict> {
        if other.operations.deltas.count() > 0 {
            return Err(AccessConflict::DeltaConflict(format!(
                "Delta conflict detected with: {}",
                other_op
            )));
        }
        Ok(())
    }

    //5. Checks for conflicts arising from remove operations on the current record.
    fn remove_conflict(
        &self,
        other: &Tracked<Status, Status>,
        other_op: String,
    ) -> Result<(), AccessConflict> {
        if other.operations.removes.count() > 0 {
            return Err(AccessConflict::RemoveConflict(format!(
                "Remove conflict detected with: {}",
                other_op
            )));
        }
        Ok(())
    }
}
