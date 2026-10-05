use std::collections::{BTreeMap, VecDeque};

use super::job_resolver::ResolvedJob;
use super::sender::Sender;

struct SenderQueue<'a> {
    sender: Sender,
    pending: VecDeque<ResolvedJob<'a>>,
}

/// Produces conflict-compatible batches while preserving each sender's nonce order.
pub(super) struct SenderBatcher<'a> {
    queues: Vec<SenderQueue<'a>>,
}

/// Provides construction and batch iteration for sender jobs.
impl<'a> SenderBatcher<'a> {
    /// Groups jobs by sender and orders each queue by nonce, then transaction ID.
    pub(super) fn new(transactions: impl IntoIterator<Item = ResolvedJob<'a>>) -> Self {
        let mut by_sender = BTreeMap::<Sender, Vec<ResolvedJob<'a>>>::new();
        for transaction in transactions {
            by_sender
                .entry(transaction.job.sender)
                .or_default()
                .push(transaction);
        }

        let queues = by_sender
            .into_iter()
            .map(|(sender, mut transactions)| {
                transactions.sort_unstable_by_key(|resolved| (resolved.job.nonce, resolved.job.id));
                SenderQueue {
                    sender,
                    pending: transactions.into(),
                }
            })
            .collect();
        Self { queues }
    }

    /// Greedily takes a conflict-compatible batch from the remaining sender queues.
    /// Visits queues by their head's conflict count, job ID, and sender address,
    /// stopping each queue at its first incompatible job. Returns `None` once
    /// all queues are empty.
    pub(super) fn next_batch(&mut self) -> Option<Vec<ResolvedJob<'a>>> {
        if self.queues.is_empty() {
            return None;
        }

        self.queues.sort_unstable_by_key(|queue| {
            let head = queue.pending.front().expect("sender queues are nonempty");
            (head.conflict_peer_count(), head.job.id, queue.sender)
        });

        let mut batch = Vec::<ResolvedJob<'a>>::new();
        for queue in &mut self.queues {
            while queue.pending.front().is_some_and(|candidate| {
                batch
                    .iter()
                    .all(|current| candidate.can_share_batch_with(current))
            }) {
                batch.push(
                    queue
                        .pending
                        .pop_front()
                        .expect("the checked sender head exists"),
                );
            }
        }

        self.queues.retain(|queue| !queue.pending.is_empty());
        Some(batch)
    }
}
