// use crate::crdt::state::StateError;
// use crate::crdt::state::counter::Counter;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Counter {
    count: u32,
}

impl Counter {
    pub(crate) fn increment(&mut self) {
        self.count += 1;
    }

    pub(crate) const fn count(&self) -> u32 {
        self.count
    }
}

impl Default for Counter {
    fn default() -> Self {
        Self { count: 0 }
    }
}

impl From<u32> for Counter {
    fn from(count: u32) -> Self {
        Self { count }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct Operations {
    pub(crate) reads: Counter,
    pub(crate) existence_checks: Counter,
    pub(crate) deltas: Counter,
    pub(crate) removes: Counter,
}
