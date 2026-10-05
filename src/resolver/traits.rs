use super::reason::Reason;
use crate::crdt::state::{Status, Tracked};
use crate::execution::output::ExecutionOutput;
use rayon::prelude::*;
use std::collections::{BTreeSet, HashMap};

pub type Rejected<K> = (ExecutionOutput<K, Tracked<Status, Status>>, Reason);

/// Checks sorted execution records for conflicts independently for each key.
pub trait Checker<K> {
    /// The record type and ordering used by this conflict check.
    type Input: Ord;

    /// Returns records rejected by this stage's checks for one state key.
    fn find_rejections_by_key(records: BTreeSet<Self::Input>) -> Vec<Rejected<K>>;

    /// Consumes each key's records in parallel and sorts rejections by
    /// transaction ID. The map retains its keys with empty record sets.
    fn find_rejections(records_by_key: &mut HashMap<K, BTreeSet<Self::Input>>) -> Vec<Rejected<K>>
    where
        K: Send + Sync,
        Self::Input: Send,
    {
        let rejected_by_key = records_by_key
            .par_iter_mut()
            .map(|(_, records)| Self::find_rejections_by_key(std::mem::take(records)))
            .filter(|rejected| !rejected.is_empty())
            .collect::<Vec<Vec<Rejected<K>>>>();

        let mut rejected = rejected_by_key.into_iter().flatten().collect::<Vec<_>>();
        rejected.sort_unstable_by_key(|(record, _)| record.id);
        rejected
    }
}
