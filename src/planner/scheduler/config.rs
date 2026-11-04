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

pub trait Scheduler<Tx> {
    fn schedule(&self, transactions: impl IntoIterator<Item = Job<Tx>>) -> Vec<Generation<Tx>>;
}
