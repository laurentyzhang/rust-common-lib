use std::collections::BTreeMap;

use super::draft::DraftPlan;
use super::job_resolver::JobResolver;
use super::sender_batcher::SenderBatcher;
use super::store::CalleeProfile;
use super::workload::{Generation, Job};

/// Options for the scheduler's planning passes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerConfig {
    /// Enable is_deferred execution for callees whose profiles support it.
    pub deferral_enabled: bool,
    /// Maximum transaction count in a generation considered for compaction.
    /// Zero disables compaction.
    pub merge_threshold: usize,
}

impl Default for SchedulerConfig {
    /// Enables deferral and sets the generation compaction threshold to 16 jobs.
    fn default() -> Self {
        Self {
            deferral_enabled: true,
            merge_threshold: 16,
        }
    }
}

/// Greedy transaction planner driven by caller-supplied callee profiles.
///
/// Profile loading, persistence, and execution belong to the caller. Each call
/// resolves a consistent policy snapshot and finalizes nonce setup only after
/// all changes to the plan's layout are complete.
#[derive(Default)]
pub struct Scheduler {
    profiles: BTreeMap<u64, CalleeProfile>,
    config: SchedulerConfig,
}

impl Scheduler {
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

    /// Builds and compacts an execution plan using the current callee profiles.
    /// Orders each sender's jobs by nonce, then ID, and applies the configured
    /// deferral rules. Finalizes each sequence's nonce offsets after compaction.
    pub fn schedule(&self, transactions: impl IntoIterator<Item = Job>) -> Vec<Generation> {
        let policy = JobResolver::new(&self.profiles);
        let mut batches = SenderBatcher::new(
            transactions
                .into_iter()
                .map(|transaction| policy.resolve(transaction)),
        );
        let mut plan = DraftPlan::default();
        while let Some(batch) = batches.next_batch() {
            plan.push_batch(batch, &self.config);
        }
        plan.compact(&self.config);
        plan.finalize()
    }
}
