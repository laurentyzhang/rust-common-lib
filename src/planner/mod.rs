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
use resolver::{CommitVerdict, ConflictDetector, Reason};
use scheduler::{CalleeProfile, GreedyScheduler, Job, Scheduler, SchedulerConfig};

/// Scheduling and conflict detection state for one generation.
pub struct Planner<DB: Database, K> {
    pub db: DB,
    pub scheduler: GreedyScheduler,
    pub checker: ConflictDetector<K>,
    callees: BTreeMap<u64, Option<CalleeId>>,
    meta_lookup: BTreeSet<Meta>,
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
            meta_lookup: BTreeSet::new(),
        }
    }

    /// Schedules deposits first in block order, then schedules the remaining jobs.
    /// Loads each function's contract meta_lookup before building the plan.
    pub fn plan<Tx>(
        &mut self,
        jobs: impl IntoIterator<Item = Job<Tx>>,
    ) -> Result<Block<Tx>, BlockExecutionError> {
        let jobs = jobs.into_iter().collect::<Vec<_>>();
        let mut callees = BTreeSet::new();
        let mut job_callees = BTreeMap::new();
        for job in &jobs {
            let callee = job
                .contract
                .map(|contract| CalleeId::new(contract, job.meta.id.selector));
            if let Some(callee) = callee {
                callees.insert(callee);
            }
            job_callees.insert(job.id.global, callee);
        }
        let mut meta_lookup = BTreeSet::new();
        for id in callees {
            let meta = match Meta::get_by_id(&meta_lookup, id) {
                Some(meta) => meta,
                None => {
                    let meta = Meta::load(&mut self.db, id.account, id.selector)?;
                    meta_lookup.insert(meta);
                    Meta::get_by_id(&meta_lookup, id).unwrap()
                }
            };
            self.scheduler.set_profile(
                id.compact,
                CalleeProfile {
                    conflict_peers: meta.conflict_peers.clone(),
                    sequential_only: meta.sequential_only,
                    deferrable: meta.deferred_gas > 0,
                },
            );
        }
        self.meta_lookup = meta_lookup;

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

    /// Converts verdict conflicts into meta_lookup changes for the caller to commit.
    pub fn build_conflict_meta_updates<'a>(
        &mut self,
        verdicts: impl IntoIterator<Item = &'a CommitVerdict<K>>,
        current: &EvmState,
    ) -> Result<EvmState, BlockExecutionError>
    where
        K: 'a,
    {
        let mut updates = EvmState::default();
        for verdict in verdicts {
            // Map the verdict's job ID to the callee recorded during planning.
            let left = self.callees.get(&verdict.id.global).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;

            // Do pairwise conflict recording for each party involved in the verdict.
            for (right_id, reason) in &verdict.conflicts {
                // Learn only from direct access conflicts between distinct jobs.
                if *right_id == verdict.id || !matches!(reason, Reason::AccessConflict(_, _)) {
                    continue;
                }
                let right = self.callees.get(&right_id.global).ok_or_else(|| {
                    BlockExecutionError::msg("conflicting transaction was not scheduled")
                })?;
                if let (Some(left), Some(right)) = (*left, *right) {
                    let mut left = Meta::take_by_id(&mut self.meta_lookup, left).unwrap();
                    let mut right = if left.id == right {
                        None
                    } else {
                        Some(Meta::take_by_id(&mut self.meta_lookup, right).unwrap())
                    };

                    // Record new conflict peers for the meta entry and its counterpart.
                    // Write the updated an evm state with the new conflict information.
                    let result = left.add_conflict_peers(
                        right.as_mut(),
                        &mut self.db,
                        current,
                        &mut updates,
                    );

                    // Reinsert the updated meta entries back into the meta_lookup set.
                    self.meta_lookup.insert(left);
                    if let Some(right) = right {
                        self.meta_lookup.insert(right);
                    }
                    result?;
                }
            }
        }
        Ok(updates)
    }
}

#[cfg(test)]
#[path = "planner_tests.rs"]
mod tests;
