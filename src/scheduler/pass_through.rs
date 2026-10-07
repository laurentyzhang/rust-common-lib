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

impl<Tx> Scheduler<Tx> for PassThroughScheduler {
    /// Places each job in its own sequence within one generation.
    fn schedule(&self, jobs: impl IntoIterator<Item = Job<Tx>>) -> Vec<Generation<Tx>> {
        let sequences = jobs
            .into_iter()
            .enumerate()
            .map(|(index, mut job)| {
                job.id = index as u64;
                JobSequence::new(vec![job])
            })
            .collect::<Vec<_>>();

        if sequences.is_empty() {
            Vec::new()
        } else {
            vec![Generation::new(sequences)]
        }
    }
}
