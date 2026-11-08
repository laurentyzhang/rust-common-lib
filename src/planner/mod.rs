pub mod meta;
pub mod resolver;
pub mod scheduler;

use std::collections::{BTreeMap, BTreeSet};

use crate::execution::workload::Block;
use alloy_evm::{block::BlockExecutionError, revm::Database};
use alloy_primitives::{Address, U256};

use meta::property::Property;
use resolver::ConflictDetector;
use scheduler::{GreedyScheduler, Job, Scheduler, SchedulerConfig};

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

    /// Loads one function's compact conflict peers or its sequential-only marker.
    fn load_conflicts(
        &mut self,
        host: Address,
        callee: u64,
    ) -> Result<(bool, BTreeSet<u64>), BlockExecutionError> {
        let count = self
            .db
            .storage(host, Property::ConflictCount.slot(host, callee, 0))
            .map_err(BlockExecutionError::other)?;
        if count == U256::MAX {
            return Ok((true, BTreeSet::new()));
        }
        let count: u64 = count
            .try_into()
            .map_err(|_| BlockExecutionError::msg("conflict count exceeds u64"))?;
        if count.div_ceil(4) > u64::from(u32::MAX) + 1 {
            return Err(BlockExecutionError::msg("conflict slot index exceeds u32"));
        }
        let mut peers = BTreeSet::new();
        for index in 0..count.div_ceil(4) {
            let value = self
                .db
                .storage(
                    host,
                    Property::ConflictIndex.slot(host, callee, index as u32),
                )
                .map_err(BlockExecutionError::other)?;
            for peer in value
                .to_be_bytes::<32>()
                .chunks_exact(8)
                .take((count - index * 4).min(4) as usize)
            {
                peers.insert(u64::from_le_bytes(peer.try_into().unwrap()));
            }
        }
        Ok((false, peers))
    }

    /// Loads each function's contract metadata before building the plan.
    pub fn plan<Tx>(
        &mut self,
        jobs: impl IntoIterator<Item = Job<Tx>>,
    ) -> Result<Block<Tx>, BlockExecutionError> {
        let jobs = jobs.into_iter().collect::<Vec<_>>();
        let callees = jobs
            .iter()
            .filter_map(|job| job.contract.zip(job.callee))
            .collect::<BTreeSet<_>>();
        for (host, callee) in callees {
            let deferred_gas = self
                .db
                .storage(host, Property::DeferredGas.slot(host, callee, 0))
                .map_err(BlockExecutionError::other)?;
            let deferred_gas: u64 = deferred_gas
                .try_into()
                .map_err(|_| BlockExecutionError::msg("deferred gas exceeds u64"))?;
            self.scheduler.set_defer(callee, deferred_gas > 0);
            let (sequential_only, peers) = self.load_conflicts(host, callee)?;
            self.scheduler.add_conflict_peers(callee, peers);
            if sequential_only {
                self.scheduler.set_sequential_only(callee);
            }
        }

        self.callees = jobs
            .iter()
            .map(|job| (job.id.global, job.contract.zip(job.callee)))
            .collect();

        // Build the schedule based on the collected conflict information.
        Ok(Block {
            generations: self.scheduler.schedule(jobs),
        })
    }
}

#[cfg(test)]
mod tests;
