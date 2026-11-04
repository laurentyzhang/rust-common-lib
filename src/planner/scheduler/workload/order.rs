use super::ID;
use std::cmp::Ordering;

/// Identifies and orders jobs by generation, sequence, and position.
#[derive(Clone, Copy, Debug, Eq)]
pub struct ScheduleKey(pub ID);

impl PartialEq for ScheduleKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Ord for ScheduleKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0
            .generation
            .cmp(&other.0.generation)
            .then_with(|| self.0.sequence.cmp(&other.0.sequence))
            .then_with(|| self.0.job.cmp(&other.0.job))
    }
}

impl PartialOrd for ScheduleKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
