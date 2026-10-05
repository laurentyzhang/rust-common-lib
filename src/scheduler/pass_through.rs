use super::scheduler::Scheduler;
use super::workload::{Generation, Job, JobSequence};

/// Passes jobs through in one generation with each job in its own sequence.
#[derive(Clone, Copy, Debug, Default)]
pub struct PassThroughScheduler;

impl PassThroughScheduler {
    /// Creates a pass-through scheduler.
    pub fn new() -> Self {
        Self
    }
}

impl Scheduler for PassThroughScheduler {
    fn schedule(&self, transactions: impl IntoIterator<Item = Job>) -> Vec<Generation> {
        let sequences = transactions
            .into_iter()
            .map(|transaction| JobSequence::new(vec![transaction]))
            .collect::<Vec<_>>();

        if sequences.is_empty() {
            Vec::new()
        } else {
            vec![Generation::new(sequences)]
        }
    }
}
