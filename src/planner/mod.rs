pub mod meta;
pub mod resolver;
pub mod scheduler;

use std::collections::{BTreeMap, BTreeSet};

use crate::execution::workload::{Block, Generation, JobSequence};
use alloy_evm::{
    block::BlockExecutionError,
    revm::{Database, state::EvmState},
};

use meta::{CalleeId, Meta};
use resolver::{CommitVerdict, ConflictDetector};
use scheduler::{CalleeProfile, GreedyScheduler, Job, Scheduler, SchedulerConfig};

type LoadedMetadata = BTreeSet<Meta>;

/// Scheduling and conflict detection state for one generation.
pub struct Planner<DB: Database, K> {
    pub db: DB,
    pub scheduler: GreedyScheduler,
    pub checker: ConflictDetector<K>,
    callees: BTreeMap<u64, Option<(CalleeId, u64)>>,
    metadata: LoadedMetadata,
}

// Planner manages the scheduling and conflict detection for transactions within a generation.
impl<DB: Database, K> Planner<DB, K> {
    /// Creates a planner with the supplied database and scheduler configuration.
    pub fn new(db: DB, config: SchedulerConfig) -> Self {
        Self {
            db,
            scheduler: GreedyScheduler::with_config(config),
            checker: ConflictDetector::new(),
            callees: BTreeMap::new(),
            metadata: BTreeSet::new(),
        }
    }

    /// Schedules deposits first in block order, then schedules the remaining jobs.
    /// Loads each function's contract metadata before building the plan.
    pub fn plan<Tx>(
        &mut self,
        jobs: impl IntoIterator<Item = Job<Tx>>,
    ) -> Result<Block<Tx>, BlockExecutionError> {
        let jobs = jobs.into_iter().collect::<Vec<_>>();
        let mut callees = BTreeSet::new();
        let mut job_callees = BTreeMap::new();
        for job in &jobs {
            let callee = job.contract.map(|contract| {
                (
                    CalleeId::new(contract, job.meta.id.selector),
                    job.meta.id.compact(),
                )
            });
            if let Some(callee) = callee {
                callees.insert(callee);
            }
            job_callees.insert(job.id.global, callee);
        }
        let mut metadata = LoadedMetadata::new();
        for (id, callee) in callees {
            let meta = match Meta::get_by_id(&metadata, id) {
                Some(meta) => meta,
                None => {
                    let meta = Meta::load(&mut self.db, id.account, id.selector)?;
                    metadata.insert(meta);
                    Meta::get_by_id(&metadata, id).unwrap()
                }
            };
            self.scheduler.set_profile(
                callee,
                CalleeProfile {
                    conflict_peers: meta.conflict_peers.clone(),
                    sequential_only: meta.sequential_only,
                    deferrable: meta.deferred_gas > 0,
                },
            );
        }
        self.metadata = metadata;

        self.callees = job_callees;

        let (mut deposits, other): (Vec<_>, Vec<_>) =
            jobs.into_iter().partition(|job| job.is_l1_deposit);
        let mut generations = self.scheduler.schedule(other);
        if !deposits.is_empty() {
            deposits.sort_by_key(|job| job.id.global);
            generations.insert(0, Generation::new(vec![JobSequence::new(deposits)]));
            for (index, generation) in generations.iter_mut().enumerate() {
                generation.assign_ids(index as u64);
            }
        }
        Ok(Block { generations })
    }

    /// Maps job conflicts to scheduled callees and returns metadata updates for the caller to commit.
    pub fn resolve<'a>(
        &mut self,
        verdicts: impl IntoIterator<Item = &'a CommitVerdict<K>>,
        current: &EvmState,
    ) -> Result<EvmState, BlockExecutionError>
    where
        K: 'a,
    {
        // Aggregate unique peer job IDs per callee across all verdicts.
        let mut peers = BTreeMap::<Meta, BTreeSet<u64>>::new();
        for verdict in verdicts {
            let left = self.callees.get(&verdict.id.global).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;
            for right in verdict.conflicts.iter().flat_map(|(peers, _)| peers) {
                let right = self.callees.get(&right.global).ok_or_else(|| {
                    BlockExecutionError::msg("conflicting transaction was not scheduled")
                })?;
                if let (Some((left_id, left)), Some((right_id, right))) = (*left, *right) {
                    // Record each conflict for both callees.
                    for (id, peer) in [(left_id, right), (right_id, left)] {
                        let meta = Meta::get_by_id(&self.metadata, id).unwrap();
                        if let Some(detected) = peers.get_mut(meta) {
                            detected.insert(peer);
                        } else {
                            peers.insert(meta.clone(), BTreeSet::from([peer]));
                        }
                    }
                }
            }
        }
        Meta::build_conflict_peer_storage_updates(&mut self.db, peers, current)
    }
}

#[cfg(test)]
#[path = "planner_tests.rs"]
mod tests;
