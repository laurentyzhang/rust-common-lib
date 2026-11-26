use super::{
    Checker, CommitDecision, CommitVerdict, Diagnostic, Reason,
    checkers::{access::AccessChecker, gas_limit::GasLimitChecker, sum::SumChecker},
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
use std::collections::{BTreeMap, BTreeSet};

/// Coordinates gas-limit, access, and numeric checks and combines their results.
pub struct ConflictDetector<K> {
    gas_limit_checker: GasLimitChecker,
    access_checker: AccessChecker<K>,
    i256_checker: SumChecker<K, I256>,
    u256_checker: SumChecker<K, U256>,
    gas_lookup: BTreeMap<ScheduleKey, u64>,
}

impl<K> ConflictDetector<K> {
    pub fn new() -> Self {
        Self {
            gas_limit_checker: GasLimitChecker::new(u64::MAX),
            access_checker: AccessChecker::new(),
            i256_checker: SumChecker::new(),
            u256_checker: SumChecker::new(),
            gas_lookup: BTreeMap::new(),
        }
    }

    pub fn set_gas_limit(&mut self, remaining_gas: u64) {
        self.gas_limit_checker.gas_limit = remaining_gas;
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
        self.gas_limit_checker.import(&executed_jobs);
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
    /// Accepts a single sequence directly; otherwise checks gas and conflicts.
    fn check(&mut self) -> Vec<CommitVerdict<K>> {
        let sequences = self.accumulate();

        let mut diagnostics = self.gas_limit_checker.check::<K>();
        let dropped: BTreeSet<_> = diagnostics.iter().map(|diagnostic| diagnostic.id).collect();

        // Check general accesses and signed/unsigned numeric bounds concurrently.
        let (access, (i256, u256)) = rayon::join(
            || self.access_checker.check(diagnostics.clone()),
            || {
                rayon::join(
                    || self.i256_checker.check_bounds(diagnostics.clone()),
                    || self.u256_checker.check_bounds(diagnostics.clone()),
                )
            },
        );

        // Keep all diagnostic types for selecting accepted sequence prefixes.
        diagnostics.extend(access.into_iter().chain(i256).chain(u256));

        // Only access conflicts allow selection to reject either endpoint.
        let pairs: BTreeSet<_> = diagnostics
            .iter()
            .filter(|diagnostic| matches!(&diagnostic.reason, Reason::AccessConflict(_, _)))
            .flat_map(|diagnostic| {
                diagnostic
                    .peers
                    .iter()
                    .map(move |&peer| (diagnostic.id.min(peer), diagnostic.id.max(peer)))
            })
            .collect();

        // resolve conflicts and dependencies to determine accepted jobs in each sequence.
        // downstream jobs in each sequence may be rejected based on the first failure.
        let accepted = selection::resolve_conflicts_and_dependencies(
            &sequences,
            &pairs,
            dropped
                .iter()
                .copied()
                .chain(diagnostics.iter().filter_map(|diagnostic| {
                    matches!(&diagnostic.reason, Reason::NumericError(_, _))
                        .then_some(diagnostic.id)
                })),
        );

        let mut verdicts: BTreeMap<_, _> = sequences
            .iter()
            .flatten()
            .map(|&(id, _)| {
                let verdict = CommitVerdict {
                    id,
                    decision: if dropped.contains(&id) {
                        CommitDecision::Dropped
                    } else if accepted.contains(&id) {
                        CommitDecision::Accepted
                    } else {
                        CommitDecision::Rejected
                    },
                    conflicts: Vec::new(),
                };
                (id, verdict)
            })
            .collect();

        // Keep one entry per related job and reason.
        for Diagnostic {
            id,
            peers: mut related,
            reason,
        } in diagnostics
        {
            related.sort_unstable();
            related.dedup();
            let conflicts = &mut verdicts.get_mut(&id).unwrap().conflicts;
            if related.is_empty() {
                if !conflicts
                    .iter()
                    .any(|(peer, existing)| *peer == id && *existing == reason)
                {
                    conflicts.push((id, reason));
                }
            } else {
                for peer in related {
                    if !conflicts.iter().any(|(existing_peer, existing)| {
                        *existing_peer == peer && *existing == reason
                    }) {
                        conflicts.push((peer, reason.clone()));
                    }
                }
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
                    .filter_map(move |(peer, _)| (*peer != *id).then_some((*id, *peer)))
            })
            .collect();
        for (id, peer) in links {
            let conflicts = &mut verdicts.get_mut(&peer).unwrap().conflicts;
            if !conflicts
                .iter()
                .any(|(existing_peer, _)| *existing_peer == id)
            {
                conflicts.push((id, Reason::None));
            }
        }
        // BTreeMap iteration returns verdicts ordered by job ID.
        verdicts.into_values().collect()
    }
}
