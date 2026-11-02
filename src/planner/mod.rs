pub mod meta;
pub mod resolver;
pub mod scheduler;

use std::collections::{BTreeMap, BTreeSet};

use alloy_evm::{block::BlockExecutionError, revm::Database};
use alloy_primitives::Address;

use resolver::ConflictDetector;
use scheduler::{Generation, GreedyScheduler, Job, Scheduler, SchedulerConfig};

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

    /// Loads each function's contract metadata before building the plan.
    pub fn plan<Tx>(
        &mut self,
        jobs: impl IntoIterator<Item = Job<Tx>>,
    ) -> Result<Vec<Generation<Tx>>, BlockExecutionError> {
        let jobs = jobs.into_iter().collect::<Vec<_>>();
        let callees = jobs
            .iter()
            .filter_map(|job| job.contract.zip(job.callee))
            .collect::<BTreeSet<_>>();
        for (host, callee) in callees {
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
        Ok(self.scheduler.schedule(jobs))
    }
}

#[cfg(test)]
mod tests;
