use std::collections::{BTreeMap, BTreeSet};

use super::store::CalleeProfile;
use super::workload::Job;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ExecutionMode {
    FullyParallel,
    ConflictChecked,
    Isolated,
}

#[derive(Debug)]
pub(super) struct ResolvedJob<'a> {
    pub(super) job: Job,
    pub(super) mode: ExecutionMode,
    pub(super) deferrable: bool,
    conflict_peers: &'a BTreeSet<u64>,
}

impl<'a> ResolvedJob<'a> {
    /// Returns the distinct conflict peer count used to prioritize sender queues.
    pub(super) fn conflict_peer_count(&self) -> usize {
        self.conflict_peers.len()
    }

    /// Returns whether both jobs may share a parallel batch under known policy.
    /// Isolation always prevents sharing; otherwise, a fully parallel job
    /// bypasses callee conflict checks for the pair.
    pub(super) fn can_share_batch_with(&self, other: &Self) -> bool {
        if self.mode == ExecutionMode::Isolated || other.mode == ExecutionMode::Isolated {
            return false;
        }
        if self.mode == ExecutionMode::FullyParallel || other.mode == ExecutionMode::FullyParallel {
            return true;
        }
        match (self.job.callee, other.job.callee) {
            (Some(_), Some(other)) => !self.conflict_peers.contains(&other),
            _ => true,
        }
    }
}

/// A single interpretation of caller-supplied policy for every planning stage.
pub(super) struct JobResolver<'a> {
    profiles: &'a BTreeMap<u64, CalleeProfile>,
    peers: BTreeMap<u64, BTreeSet<u64>>,
    empty_peers: BTreeSet<u64>,
}

impl<'a> JobResolver<'a> {
    /// Builds a symmetric conflict index from the supplied callee profiles.
    /// A conflict declared by either callee applies in both directions, even
    /// when the other callee has no profile.
    pub(super) fn new(profiles: &'a BTreeMap<u64, CalleeProfile>) -> Self {
        let mut peers = BTreeMap::<u64, BTreeSet<u64>>::new();
        for (&callee, profile) in profiles {
            for &peer in &profile.conflict_peers {
                peers.entry(callee).or_default().insert(peer);
                peers.entry(peer).or_default().insert(callee);
            }
        }
        Self {
            profiles,
            peers,
            empty_peers: BTreeSet::new(),
        }
    }

    /// Resolves a job's execution mode, deferral eligibility, and conflict peers.
    /// The fully parallel flag overrides this job's callee restrictions;
    /// deferral eligibility still comes from its profile.
    pub(super) fn resolve<'resolver>(&'resolver self, job: Job) -> ResolvedJob<'resolver> {
        let profile = job.callee.and_then(|callee| self.profiles.get(&callee));
        let mode = if job.fully_parallelizable {
            ExecutionMode::FullyParallel
        } else if profile.is_some_and(|profile| profile.sequential_only) {
            ExecutionMode::Isolated
        } else {
            ExecutionMode::ConflictChecked
        };
        let deferrable = profile.is_some_and(|profile| profile.deferrable);
        let conflict_peers = job
            .callee
            .and_then(|callee| self.peers.get(&callee))
            .unwrap_or(&self.empty_peers);
        ResolvedJob {
            job,
            mode,
            deferrable,
            conflict_peers,
        }
    }
}
