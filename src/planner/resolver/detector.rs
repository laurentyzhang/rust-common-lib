use super::{
    Checker, CommitVerdict, Reason,
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

    /// Checks all records accumulated by imports, consuming the current batch.
    fn check(&mut self) -> Vec<CommitVerdict<K>> {
        let ((mut diagnostics, pairs), ((i256, _), (u256, _))) = rayon::join(
            || self.access_checker.check(),
            || {
                rayon::join(
                    || self.i256_checker.check_bounds(),
                    || self.u256_checker.check_bounds(),
                )
            },
        );

        diagnostics.extend(i256.into_iter().chain(u256));
        let sequences = self.accumulate();

        let accepted = selection::select(
            &sequences,
            &pairs,
            diagnostics.iter().filter_map(|(id, _, _, reason)| {
                matches!(reason, Reason::NumericError(_)).then_some(*id)
            }),
        );

        // The first rejected job explains the discarded suffix of its sequence.
        let mut verdicts = BTreeMap::new();
        for sequence in &sequences {
            let mut dependency = None;
            for &(id, _) in sequence {
                let mut verdict = CommitVerdict {
                    id,
                    accepted: accepted.contains(&id),
                    peers: Vec::new(),
                    reasons: Vec::new(),
                };
                if !verdict.accepted {
                    if let Some(cause) = dependency {
                        verdict.reasons.push((None, Reason::Dependency(cause)));
                    } else {
                        dependency = Some(id);
                    }
                }
                verdicts.insert(id, verdict);
            }
        }

        for (id, key, related, reason) in diagnostics {
            let key = Some(key);
            for peer in related {
                verdicts.get_mut(&peer).unwrap().peers.push(id);
                verdicts.get_mut(&id).unwrap().peers.push(peer);
            }
            let verdict = verdicts.get_mut(&id).unwrap();
            if !verdict
                .reasons
                .iter()
                .any(|(existing_key, existing_reason)| {
                    existing_key == &key && existing_reason == &reason
                })
            {
                verdict.reasons.push((key, reason));
            }
        }
        for verdict in verdicts.values_mut() {
            verdict.peers.sort_unstable();
            verdict.peers.dedup();
            verdict
                .reasons
                .sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        }
        verdicts.into_values().collect()
    }
}
