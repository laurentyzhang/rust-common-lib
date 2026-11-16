use super::{
    Checker, CommitDecision, CommitVerdict, Reason,
    checkers::{access::AccessChecker, sum::SumChecker},
    selection,
};
use crate::crdt::state::{
    Tracked,
    status::Status,
    value::{CrdtValue, ValueSnapshots},
};
use crate::execution::output::ExecutionOutput;
use crate::planner::scheduler::{ID, ScheduleKey};
use alloy_primitives::{I256, U256};
use std::collections::BTreeMap;

/// Coordinates access and numeric conflict checks and combines their results.
pub struct ConflictDetector<K> {
    access_checker: AccessChecker<K>,
    i256_checker: SumChecker<K, I256>,
    u256_checker: SumChecker<K, U256>,
    gas_lookup: BTreeMap<ScheduleKey, u64>,
}

impl<K> ConflictDetector<K> {
    pub fn new() -> Self {
        Self {
            access_checker: AccessChecker::new(),
            i256_checker: SumChecker::new(),
            u256_checker: SumChecker::new(),
            gas_lookup: BTreeMap::new(),
        }
    }

    fn accumulate(&mut self) -> Vec<Vec<(ID, u64)>> {
        // Group jobs by their scheduled sequence.
        let mut sequences = Vec::<Vec<(ID, u64)>>::new();
        let mut previous = None;
        for (key, gas) in std::mem::take(&mut self.gas_lookup) {
            let id = key.0;
            let sequence = (id.block, id.generation, id.sequence);
            if previous != Some(sequence) {
                sequences.push(Vec::new());
                previous = Some(sequence);
            }
            let jobs = sequences.last_mut().unwrap();
            let cumulative = jobs.last().map_or(0, |(_, gas)| *gas) + gas;
            jobs.push((id, cumulative));
        }
        sequences
    }
}

impl<K: Clone + Ord + std::hash::Hash + Send + Sync> Checker<K> for ConflictDetector<K> {
    fn import(&mut self, executed_jobs: Vec<ExecutionOutput<K, Tracked<Status, Status>>>) {
        let mut i256_records = Vec::new();
        let mut u256_records = Vec::new();
        for job in &executed_jobs {
            self.gas_lookup
                .entry(ScheduleKey(job.id))
                .or_insert(job.gas_used); // Insert gas used for this job if not already present.

            match &job.data.value.current {
                Status::Value(CrdtValue::I256(value)) => {
                    i256_records.push(job.clone_with_data(job.data.clone_with_states(
                        ValueSnapshots::from_refs(&job.data.value.original, value.as_ref()),
                    )))
                }
                Status::Value(CrdtValue::U256(value)) => {
                    u256_records.push(job.clone_with_data(job.data.clone_with_states(
                        ValueSnapshots::from_refs(&job.data.value.original, value.as_ref()),
                    )))
                }
                _ => {}
            }
        }
        self.access_checker.import(executed_jobs);
        self.i256_checker.import(i256_records);
        self.u256_checker.import(u256_records);
    }

    /// Consumes the batch and returns job verdicts with conflict diagnostics.
    /// Accepts a single sequence directly; otherwise checks conflicts and selects sequence prefixes.
    fn check(&mut self) -> Vec<CommitVerdict<K>> {
        let sequences = self.accumulate();
        // A single sequence already ran in order; accept its jobs and clear the batch.
        if sequences.len() == 1 {
            *self = Self::new();
            let mut verdicts: Vec<_> = sequences
                .into_iter()
                .flatten()
                .map(|(id, _)| CommitVerdict {
                    id,
                    decision: CommitDecision::Accepted,
                    conflicts: Vec::new(),
                })
                .collect();
            verdicts.sort_unstable_by_key(|verdict| verdict.id);
            return verdicts;
        }

        // Check general accesses and signed/unsigned numeric bounds concurrently.
        let ((mut diagnostics, pairs), ((i256, _), (u256, _))) = rayon::join(
            || self.access_checker.check(),
            || {
                rayon::join(
                    || self.i256_checker.check_bounds(),
                    || self.u256_checker.check_bounds(),
                )
            },
        );

        // Keep all diagnostic types for selecting accepted sequence prefixes.
        diagnostics.extend(i256.into_iter().chain(u256));

        // Numeric failures are mandatory rejections; access pairs are resolved
        // by selecting compatible prefixes from the affected sequences.
        let accepted = selection::select(
            &sequences,
            &pairs,
            diagnostics.iter().filter_map(|(id, _, reason)| {
                matches!(reason, Reason::NumericError(_, _)).then_some(*id)
            }),
        );

        // Create a verdict for every inspector_records job. The first rejection in each
        // sequence explains why subsequent jobs in that sequence were rejected.
        let mut verdicts = BTreeMap::new();
        for sequence in &sequences {
            let mut dependency = None;
            for &(id, _) in sequence {
                let mut verdict = CommitVerdict {
                    id,
                    decision: if accepted.contains(&id) {
                        CommitDecision::Accepted
                    } else {
                        CommitDecision::Rejected
                    },
                    conflicts: Vec::new(),
                };
                if verdict.decision == CommitDecision::Rejected {
                    if let Some(cause) = dependency {
                        verdict
                            .conflicts
                            .push((Vec::new(), Reason::Dependency(cause)));
                    } else {
                        dependency = Some(id);
                    }
                }
                verdicts.insert(id, verdict);
            }
        }

        // Merge duplicate reasons, retaining their distinct associated peers.
        for (id, mut related, reason) in diagnostics {
            related.sort_unstable();
            related.dedup();
            let conflicts = &mut verdicts.get_mut(&id).unwrap().conflicts;
            if let Some((peers, _)) = conflicts
                .iter_mut()
                .find(|(_, existing)| *existing == reason)
            {
                peers.extend(related);
                peers.sort_unstable();
                peers.dedup();
            } else {
                conflicts.push((related, reason));
            }
        }
        // Ensure every reported peer link is visible from both endpoints, even
        // when only one endpoint received a detailed diagnostic.
        let links: Vec<_> = verdicts
            .iter()
            .flat_map(|(id, verdict)| {
                verdict
                    .conflicts
                    .iter()
                    .flat_map(move |(peers, _)| peers.iter().map(move |peer| (*id, *peer)))
            })
            .collect();
        for (id, peer) in links {
            let conflicts = &mut verdicts.get_mut(&peer).unwrap().conflicts;
            if !conflicts.iter().any(|(peers, _)| peers.contains(&id)) {
                conflicts.push((vec![id], Reason::None));
            }
        }
        // BTreeMap iteration returns verdicts ordered by job ID.
        verdicts.into_values().collect()
    }
}
