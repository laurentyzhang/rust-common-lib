use std::collections::BTreeMap;

use super::draft::DraftPlan;
use super::job_resolver::JobResolver;
use super::scheduler::{Scheduler, SchedulerConfig};
use super::sender_batcher::SenderBatcher;
use super::store::CalleeProfile;
use super::workload::{Generation, Job};

/// Greedy transaction planner driven by caller-supplied callee profiles.
///
/// Profile loading, persistence, and execution belong to the caller. Each call
/// resolves a consistent policy snapshot and finalizes nonce setup only after
/// all changes to the plan's layout are complete.
#[derive(Default)]
pub struct GreedyScheduler {
    profiles: BTreeMap<u64, CalleeProfile>,
    config: SchedulerConfig,
}

impl GreedyScheduler {
    /// Creates a scheduler with default planning options and no callee profiles.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a scheduler with the supplied planning options and no callee profiles.
    pub fn with_config(config: SchedulerConfig) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// Returns the active planning configuration.
    pub fn config(&self) -> &SchedulerConfig {
        &self.config
    }

    /// Inserts or replaces the callee profile used by subsequent scheduling calls.
    pub fn set_profile(&mut self, callee: u64, profile: CalleeProfile) {
        self.profiles.insert(callee, profile);
    }
}

impl<Tx> Scheduler<Tx> for GreedyScheduler {
    /// Builds and compacts an execution plan using the current callee profiles.
    /// Orders each sender's jobs by nonce, then ID, and applies the configured
    /// deferral rules. Finalizes each sequence's nonce offsets after compaction.
    fn schedule(&self, txs: impl IntoIterator<Item = Job<Tx>>) -> Vec<Generation<Tx>> {
        let resolver = JobResolver::new(&self.profiles);
        let mut batches = SenderBatcher::new(txs.into_iter().map(|job| resolver.resolve(job)));
        let mut plan = DraftPlan::default();
        while let Some(batch) = batches.next_batch() {
            plan.push_batch(batch, &self.config);
        }
        plan.compact(&self.config);
        plan.finalize()
    }
}
