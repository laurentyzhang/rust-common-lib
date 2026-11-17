use std::collections::BTreeSet;

/// Scheduling policy learned for one callee.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CalleeProfile {
    pub conflict_peers: BTreeSet<u64>,
    pub sequential_only: bool,
    pub deferrable: bool,
}
