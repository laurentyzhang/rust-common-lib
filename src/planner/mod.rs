pub mod meta;
pub mod resolver;
pub mod scheduler;

use std::collections::{BTreeMap, BTreeSet};

use crate::execution::workload::{Block, Generation, JobSequence};
use alloy_evm::{
    block::BlockExecutionError,
    revm::{Database, state::EvmState},
};
use alloy_primitives::Address;

use meta::Meta;
use resolver::{CommitVerdict, ConflictDetector};
use scheduler::{CalleeProfile, GreedyScheduler, Job, Scheduler, SchedulerConfig};

/// Scheduling and conflict detection state for one generation.
pub struct Planner<DB: Database, K> {
    pub db: DB,
    pub scheduler: GreedyScheduler,
    pub checker: ConflictDetector<K>,
    callees: BTreeMap<u64, Option<(Address, u64)>>,
}

// Planner manages the scheduling and conflict detection for transactions within a generation.
impl<DB: Database, K> Planner<DB, K> {
    pub fn new(db: DB, config: SchedulerConfig) -> Self {
        Self {
            db,
            scheduler: GreedyScheduler::with_config(config),
            checker: ConflictDetector::new(),
            callees: BTreeMap::new(),
        }
    }

    /// Schedules deposits first in block order, then schedules the remaining jobs.
    /// Loads each function's contract metadata before building the plan.
    pub fn plan<Tx>(
        &mut self,
        jobs: impl IntoIterator<Item = Job<Tx>>,
    ) -> Result<Block<Tx>, BlockExecutionError> {
        let jobs = jobs.into_iter().collect::<Vec<_>>();
        let callees = jobs
            .iter()
            .filter_map(|job| job.contract.zip(job.meta.id.map(|callee| callee.compact())))
            .collect::<BTreeSet<_>>();
        for (host, callee) in callees {
            let selector = callee.to_le_bytes()[4..].try_into().unwrap();
            let meta = Meta::load(&mut self.db, host, selector)?;
            self.scheduler.set_profile(
                callee,
                CalleeProfile {
                    conflict_peers: meta.conflict_peers,
                    sequential_only: meta.sequential_only,
                    deferrable: meta.deferred_gas > 0,
                },
            );
        }

        self.callees = jobs
            .iter()
            .map(|job| {
                (
                    job.id.global,
                    job.contract.zip(job.meta.id.map(|callee| callee.compact())),
                )
            })
            .collect();

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
        // Deduplicate peers across verdicts and record each conflict in both directions.
        let mut peers = BTreeMap::<(Address, [u8; 4]), BTreeSet<u64>>::new();
        for verdict in verdicts {
            let left = self.callees.get(&verdict.id.global).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;
            for right in verdict.conflicts.iter().flat_map(|(peers, _)| peers) {
                let right = self.callees.get(&right.global).ok_or_else(|| {
                    BlockExecutionError::msg("conflicting transaction was not scheduled")
                })?;
                if let (Some((left_contract, left)), Some((right_contract, right))) =
                    (*left, *right)
                {
                    let left_selector = left.to_le_bytes()[4..].try_into().unwrap();
                    let right_selector = right.to_le_bytes()[4..].try_into().unwrap();
                    peers
                        .entry((left_contract, left_selector))
                        .or_default()
                        .insert(right);
                    peers
                        .entry((right_contract, right_selector))
                        .or_default()
                        .insert(left);
                }
            }
        }
        Meta::updates(&mut self.db, peers, current)
    }
}

#[cfg(test)]
#[path = "planner_tests.rs"]
mod tests;
