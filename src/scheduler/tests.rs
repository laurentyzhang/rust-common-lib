use super::sender::{Sender, SenderNonceOffset};
use super::*;
use std::collections::{BTreeMap, BTreeSet};

fn transaction(id: u64, sender: u8, nonce: u64, callee: u64) -> Job {
    let mut address = [0; 20];
    address[19] = sender;
    Job {
        id,
        sender: address,
        nonce,
        callee: Some(callee),
        is_deferred: false,
        fully_parallelizable: false,
    }
}

fn ids(generation: &Generation) -> Vec<u64> {
    let mut ids = generation
        .sequences
        .iter()
        .flat_map(|sequence| {
            sequence
                .transactions
                .iter()
                .map(|transaction| transaction.id)
        })
        .collect::<Vec<_>>();
    ids.sort_unstable();
    ids
}

#[test]
fn unrelated_callees_share_a_parallel_generation() {
    let mut scheduler = GreedyScheduler::new();
    scheduler.set_profile(1, CalleeProfile::default());
    scheduler.set_profile(2, CalleeProfile::default());

    let plan = scheduler.schedule([
        transaction(1, 1, 0, 1),
        transaction(2, 2, 0, 2),
        transaction(3, 3, 0, 1),
    ]);

    assert_eq!(plan.len(), 1);
    assert_eq!(ids(&plan[0]), vec![1, 2, 3]);
}

#[test]
fn known_conflicting_callees_are_serialized_during_finalization() {
    let mut scheduler = GreedyScheduler::new();
    scheduler.set_profile(
        1,
        CalleeProfile {
            conflict_peers: BTreeSet::from([2]),
            ..CalleeProfile::default()
        },
    );
    scheduler.set_profile(
        2,
        CalleeProfile {
            conflict_peers: BTreeSet::from([1]),
            ..CalleeProfile::default()
        },
    );

    let plan = scheduler.schedule([transaction(1, 1, 0, 1), transaction(2, 2, 0, 2)]);

    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].sequences.len(), 1);
    assert_eq!(ids(&plan[0]), vec![1, 2]);
}

#[test]
fn sequential_only_callee_runs_alone() {
    let mut scheduler = GreedyScheduler::new();
    scheduler.set_profile(
        1,
        CalleeProfile {
            sequential_only: true,
            ..CalleeProfile::default()
        },
    );
    scheduler.set_profile(2, CalleeProfile::default());

    let plan = scheduler.schedule([transaction(1, 1, 0, 1), transaction(2, 2, 0, 2)]);

    assert_eq!(plan.len(), 2);
    assert_eq!(ids(&plan[0]), vec![1]);
    assert_eq!(ids(&plan[1]), vec![2]);
}

#[test]
fn repeated_deferrable_callee_places_one_call_in_next_generation() {
    let mut scheduler = GreedyScheduler::new();
    scheduler.set_profile(
        1,
        CalleeProfile {
            deferrable: true,
            ..CalleeProfile::default()
        },
    );

    let plan = scheduler.schedule([transaction(1, 1, 0, 1), transaction(2, 2, 0, 1)]);

    assert_eq!(plan.len(), 2);
    assert_eq!(ids(&plan[0]), vec![1]);
    assert_eq!(ids(&plan[1]), vec![2]);
    assert!(plan[1].sequences[0].transactions[0].is_deferred);
}

#[test]
fn disabling_deferral_keeps_repeated_deferrable_calls_in_current_generation() {
    let mut scheduler = GreedyScheduler::with_config(SchedulerConfig {
        deferral_enabled: false,
        ..SchedulerConfig::default()
    });
    scheduler.set_profile(
        1,
        CalleeProfile {
            deferrable: true,
            ..CalleeProfile::default()
        },
    );

    let plan = scheduler.schedule([transaction(1, 1, 0, 1), transaction(2, 2, 0, 1)]);

    assert_eq!(plan.len(), 1);
    assert_eq!(ids(&plan[0]), vec![1, 2]);
    assert!(
        plan[0]
            .sequences
            .iter()
            .flat_map(|sequence| &sequence.transactions)
            .all(|transaction| !transaction.is_deferred)
    );
}

#[test]
fn sender_nonce_offsets_survive_sequence_id_sorting() {
    let mut scheduler = GreedyScheduler::new();
    scheduler.set_profile(1, CalleeProfile::default());

    let plan = scheduler.schedule([transaction(10, 1, 1, 1), transaction(20, 1, 0, 1)]);

    assert_eq!(ids(&plan[0]), vec![10, 20]);
    let sequence_nonce_offsets = plan[0]
        .sequences
        .iter()
        .map(|sequence| {
            let transaction = &sequence.transactions[0];
            assert_eq!(sequence.nonce_offsets.len(), 1);
            assert_eq!(sequence.nonce_offsets[0].sender, transaction.sender);
            (transaction.id, sequence.nonce_offsets[0].offset)
        })
        .collect::<Vec<_>>();
    assert_eq!(sequence_nonce_offsets, vec![(10, 1), (20, 0)]);
}

fn uncompacted_scheduler() -> GreedyScheduler {
    GreedyScheduler::with_config(SchedulerConfig {
        merge_threshold: 0,
        ..SchedulerConfig::default()
    })
}

fn location(plan: &[Generation], id: u64) -> (usize, usize, usize) {
    for (generation_index, generation) in plan.iter().enumerate() {
        for (sequence_index, sequence) in generation.sequences.iter().enumerate() {
            for (transaction_index, transaction) in sequence.transactions.iter().enumerate() {
                if transaction.id == id {
                    return (generation_index, sequence_index, transaction_index);
                }
            }
        }
    }
    panic!("transaction {id} is missing from the plan");
}

fn assert_serial_order(plan: &[Generation], before: u64, after: u64) {
    let before = location(plan, before);
    let after = location(plan, after);
    assert!(
        before.0 < after.0 || (before.0 == after.0 && before.1 == after.1 && before.2 < after.2),
        "transactions must execute in order: {before:?} before {after:?}; plan: {plan:?}"
    );
}

fn triangle_profiles(scheduler: &mut GreedyScheduler) {
    scheduler.set_profile(
        1,
        CalleeProfile {
            conflict_peers: BTreeSet::from([2, 3]),
            ..CalleeProfile::default()
        },
    );
    scheduler.set_profile(
        2,
        CalleeProfile {
            conflict_peers: BTreeSet::from([3]),
            deferrable: true,
            ..CalleeProfile::default()
        },
    );
}

#[test]
fn asymmetric_conflicts_are_respected_after_sender_heads_are_consumed() {
    let mut scheduler = uncompacted_scheduler();
    scheduler.set_profile(
        1,
        CalleeProfile {
            conflict_peers: BTreeSet::from([2]),
            ..CalleeProfile::default()
        },
    );

    let plan = scheduler.schedule([
        transaction(1, 1, 0, 3),
        transaction(2, 1, 1, 1),
        transaction(3, 2, 0, 4),
        transaction(4, 2, 1, 2),
    ]);

    assert_eq!(plan.len(), 2);
    assert_eq!(ids(&plan[0]), vec![1, 2, 3]);
    assert_eq!(ids(&plan[1]), vec![4]);
}

#[test]
fn fully_parallelizable_overrides_its_own_isolation_and_conflicts() {
    let mut scheduler = uncompacted_scheduler();
    scheduler.set_profile(
        1,
        CalleeProfile {
            conflict_peers: BTreeSet::from([2]),
            sequential_only: true,
            ..CalleeProfile::default()
        },
    );
    let mut parallel = transaction(1, 1, 0, 1);
    parallel.fully_parallelizable = true;

    let plan = scheduler.schedule([parallel, transaction(2, 2, 0, 2)]);

    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].sequences.len(), 2);
}

#[test]
fn isolated_transaction_stays_alone_regardless_of_parallel_peer_seed_order() {
    for (isolated_id, parallel_id) in [(1, 2), (2, 1)] {
        let mut scheduler = GreedyScheduler::new();
        scheduler.set_profile(
            1,
            CalleeProfile {
                sequential_only: true,
                ..CalleeProfile::default()
            },
        );
        let mut parallel = transaction(parallel_id, 2, 0, 2);
        parallel.fully_parallelizable = true;

        let plan = scheduler.schedule([transaction(isolated_id, 1, 0, 1), parallel]);

        assert_eq!(plan.len(), 2);
        assert!(
            plan
                .iter()
                .all(|generation| ids(generation).len() == 1)
        );
    }
}

#[test]
fn replacing_a_profile_removes_its_stale_incoming_conflict_edges() {
    let mut scheduler = uncompacted_scheduler();
    scheduler.set_profile(
        1,
        CalleeProfile {
            conflict_peers: BTreeSet::from([2]),
            ..CalleeProfile::default()
        },
    );
    let transactions = [transaction(1, 1, 0, 1), transaction(2, 2, 0, 2)];
    assert_eq!(
        scheduler.schedule(transactions.clone()).len(),
        2
    );

    scheduler.set_profile(1, CalleeProfile::default());
    let plan = scheduler.schedule(transactions);

    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].sequences.len(), 2);
}

#[test]
fn compaction_preserves_sender_order_across_a_deferred_predecessor() {
    let mut scheduler = GreedyScheduler::new();
    triangle_profiles(&mut scheduler);

    let plan = scheduler.schedule([
        transaction(1, 1, 0, 1),
        transaction(2, 2, 0, 2),
        transaction(3, 2, 1, 3),
    ]);

    assert_serial_order(&plan, 1, 2);
    assert_serial_order(&plan, 2, 3);
}

#[test]
fn compaction_does_not_leave_a_sender_predecessor_in_the_source_generation() {
    for (predecessor_id, successor_id) in [(3, 4), (4, 3)] {
        let profiles = BTreeMap::from([
            (1, BTreeSet::from([3, 4])),
            (2, BTreeSet::from([3])),
            (3, BTreeSet::from([1, 2])),
            (4, BTreeSet::from([1])),
        ])
        .into_iter()
        .map(|(callee, conflict_peers)| {
            (
                callee,
                CalleeProfile {
                    conflict_peers,
                    ..CalleeProfile::default()
                },
            )
        })
        .collect::<BTreeMap<_, _>>();
        let mut scheduler = GreedyScheduler::new();
        for (&callee, profile) in &profiles {
            scheduler.set_profile(callee, profile.clone());
        }
        let transactions = [
            transaction(1, 1, 0, 1),
            transaction(2, 2, 0, 2),
            transaction(predecessor_id, 3, 0, 3),
            transaction(successor_id, 3, 1, 4),
        ];

        let plan = scheduler.schedule(transactions.clone());

        // The predecessor conflicts with both destination sequences and must
        // stay in the source; its successor must not merge ahead of it.
        // Sharing a generation in separate sequences remains valid.
        assert!(
            location(&plan, predecessor_id).0 <= location(&plan, successor_id).0,
            "compaction moved a sender's successor before its predecessor: {plan:?}"
        );
        assert_plan_invariants(&transactions, &profiles, &plan);
    }
}

#[test]
fn compaction_does_not_jump_over_an_intervening_conflicting_callee() {
    let mut scheduler = GreedyScheduler::new();
    triangle_profiles(&mut scheduler);

    let plan = scheduler.schedule([
        transaction(1, 1, 0, 1),
        transaction(2, 2, 0, 2),
        transaction(3, 3, 0, 3),
    ]);

    assert_serial_order(&plan, 1, 2);
    assert_serial_order(&plan, 2, 3);
}

#[test]
fn compaction_does_not_jump_over_an_isolated_transaction() {
    let mut scheduler = GreedyScheduler::new();
    scheduler.set_profile(
        1,
        CalleeProfile {
            conflict_peers: BTreeSet::from([3]),
            ..CalleeProfile::default()
        },
    );
    scheduler.set_profile(
        2,
        CalleeProfile {
            sequential_only: true,
            ..CalleeProfile::default()
        },
    );

    let plan = scheduler.schedule([
        transaction(1, 1, 0, 1),
        transaction(2, 1, 1, 2),
        transaction(3, 2, 0, 3),
    ]);

    assert_eq!(plan.len(), 3);
    assert_eq!(ids(&plan[0]), vec![1]);
    assert_eq!(ids(&plan[1]), vec![2]);
    assert_eq!(ids(&plan[2]), vec![3]);
}

#[test]
fn deferral_preserves_sender_order_when_ids_disagree_with_nonces() {
    let mut scheduler = GreedyScheduler::new();
    scheduler.set_profile(
        1,
        CalleeProfile {
            deferrable: true,
            ..CalleeProfile::default()
        },
    );

    let plan = scheduler.schedule([
        transaction(20, 1, 0, 1),
        transaction(10, 1, 1, 1),
        transaction(30, 1, 2, 2),
    ]);

    assert!(location(&plan, 20).0 <= location(&plan, 10).0);
    assert!(location(&plan, 10).0 <= location(&plan, 30).0);
    let marked_deferred = plan
        
        .iter()
        .flat_map(|generation| &generation.sequences)
        .flat_map(|sequence| &sequence.transactions)
        .filter(|transaction| transaction.is_deferred)
        .map(|transaction| transaction.id)
        .collect::<Vec<_>>();
    assert_eq!(marked_deferred, vec![20]);
}

#[test]
fn a_merged_sequence_has_one_nonce_offset_for_each_sender() {
    let mut scheduler = GreedyScheduler::new();
    scheduler.set_profile(
        1,
        CalleeProfile {
            conflict_peers: BTreeSet::from([2]),
            ..CalleeProfile::default()
        },
    );
    let first = transaction(1, 1, 0, 1);
    let second = transaction(2, 2, 0, 2);

    let plan = scheduler.schedule([first.clone(), second.clone()]);

    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].sequences.len(), 1);
    assert_eq!(
        plan[0].sequences[0].nonce_offsets,
        vec![
            SenderNonceOffset {
                sender: first.sender,
                offset: 0,
            },
            SenderNonceOffset {
                sender: second.sender,
                offset: 0,
            },
        ]
    );
}

#[test]
fn empty_input_produces_an_empty_plan() {
    assert_eq!(GreedyScheduler::new().schedule([]), Vec::<Generation>::new());
}

#[test]
fn pass_through_scheduler_puts_each_job_in_its_own_parallel_sequence() {
    let plan = PassThroughScheduler::new().schedule([
        transaction(1, 1, 0, 1),
        transaction(2, 2, 0, 2),
        transaction(3, 1, 1, 3),
    ]);

    assert_eq!(plan.len(), 1);
    assert_eq!(ids(&plan[0]), vec![1, 2, 3]);
    assert!(
        plan[0]
            .sequences
            .iter()
            .all(|sequence| sequence.transactions.len() == 1)
    );
    let offsets = plan[0]
        .sequences
        .iter()
        .map(|sequence| {
            let job = &sequence.transactions[0];
            (job.id, sequence.nonce_offsets[0].offset)
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(offsets.get(&1), Some(&0));
    assert_eq!(offsets.get(&2), Some(&0));
    assert_eq!(offsets.get(&3), Some(&1));
}

#[test]
fn pass_through_scheduler_returns_no_generations_for_empty_input() {
    assert_eq!(PassThroughScheduler::new().schedule([]), Vec::<Generation>::new());
}

#[test]
fn input_permutations_produce_the_same_plan() {
    let mut scheduler = GreedyScheduler::new();
    triangle_profiles(&mut scheduler);
    let mut transactions = vec![
        transaction(6, 1, 0, 1),
        transaction(5, 1, 1, 2),
        transaction(4, 1, 2, 3),
        transaction(3, 2, 0, 1),
        transaction(2, 2, 1, 3),
        transaction(1, 3, 0, 2),
    ];
    let expected = scheduler.schedule(transactions.clone());

    for _ in 0..transactions.len() {
        transactions.rotate_left(1);
        assert_eq!(scheduler.schedule(transactions.clone()), expected);
        transactions.reverse();
        assert_eq!(scheduler.schedule(transactions.clone()), expected);
        transactions.reverse();
    }
}
fn assert_plan_invariants(
    input: &[Job],
    profiles: &BTreeMap<u64, CalleeProfile>,
    plan: &[Generation],
) {
    let inputs_by_id = input
        .iter()
        .map(|transaction| (transaction.id, transaction))
        .collect::<BTreeMap<_, _>>();
    let mut actual_ids = plan.iter().flat_map(ids).collect::<Vec<_>>();
    actual_ids.sort_unstable();
    assert_eq!(actual_ids, inputs_by_id.keys().copied().collect::<Vec<_>>());

    let mut previous_sender_nonces = BTreeMap::new();
    for generation in plan {
        assert!(!generation.sequences.is_empty());
        let mut sender_order = BTreeMap::<Sender, Vec<(u64, u64)>>::new();
        for sequence in &generation.sequences {
            assert!(!sequence.transactions.is_empty());
            for scheduled in &sequence.transactions {
                let original = inputs_by_id[&scheduled.id];
                assert_eq!(scheduled.sender, original.sender);
                assert_eq!(scheduled.nonce, original.nonce);
                assert_eq!(scheduled.callee, original.callee);
                sender_order
                    .entry(scheduled.sender)
                    .or_default()
                    .push((scheduled.nonce, scheduled.id));
                if !original.fully_parallelizable
                    && original
                        .callee
                        .and_then(|callee| profiles.get(&callee))
                        .is_some_and(|profile| profile.sequential_only)
                {
                    assert_eq!(ids(generation), vec![scheduled.id]);
                }
            }
        }
        for (sender, transactions) in &mut sender_order {
            transactions.sort_unstable();
            if let Some(previous) =
                previous_sender_nonces.insert(*sender, *transactions.last().unwrap())
            {
                assert!(
                    previous < transactions[0],
                    "sender nonce order reversed across generations: {plan:?}"
                );
            }
        }

        for (sequence_index, sequence) in generation.sequences.iter().enumerate() {
            let mut expected_offsets = BTreeMap::new();
            let mut previous_ranks = BTreeMap::new();
            for scheduled in &sequence.transactions {
                let rank = sender_order[&scheduled.sender]
                    .iter()
                    .position(|key| *key == (scheduled.nonce, scheduled.id))
                    .unwrap() as u64;
                expected_offsets.entry(scheduled.sender).or_insert(rank);
                if let Some(previous) = previous_ranks.insert(scheduled.sender, rank) {
                    assert_eq!(
                        rank,
                        previous + 1,
                        "a sequence must contain a contiguous nonce span for each sender: {plan:?}"
                    );
                }
            }
            let actual_offsets = sequence
                .nonce_offsets
                .iter()
                .map(|offset| (offset.sender, offset.offset))
                .collect::<BTreeMap<_, _>>();
            assert_eq!(sequence.nonce_offsets.len(), actual_offsets.len());
            assert_eq!(actual_offsets, expected_offsets);

            for other_sequence in &generation.sequences[sequence_index + 1..] {
                for left in &sequence.transactions {
                    for right in &other_sequence.transactions {
                        let left = inputs_by_id[&left.id];
                        let right = inputs_by_id[&right.id];
                        if left.fully_parallelizable || right.fully_parallelizable {
                            continue;
                        }
                        if let (Some(left_callee), Some(right_callee)) = (left.callee, right.callee)
                        {
                            let conflict = profiles.get(&left_callee).is_some_and(|profile| {
                                profile.conflict_peers.contains(&right_callee)
                            }) || profiles.get(&right_callee).is_some_and(
                                |profile| profile.conflict_peers.contains(&left_callee),
                            );
                            assert!(!conflict, "conflicting callees run in parallel: {plan:?}");
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn small_profile_combinations_preserve_public_plan_invariants() {
    for conflict_mask in 0..8 {
        for isolated_callee in 0..=3 {
            for deferral_enabled in [false, true] {
                for fully_parallelizable in [false, true] {
                    for merge_threshold in [0, 16] {
                        let mut profiles = (1..=3)
                            .map(|callee| {
                                (
                                    callee,
                                    CalleeProfile {
                                        sequential_only: callee == isolated_callee,
                                        deferrable: callee != 3,
                                        ..CalleeProfile::default()
                                    },
                                )
                            })
                            .collect::<BTreeMap<_, _>>();
                        for (bit, (left, right)) in [(1, 2), (1, 3), (2, 3)].into_iter().enumerate()
                        {
                            if conflict_mask & (1 << bit) != 0 {
                                profiles
                                    .get_mut(&left)
                                    .unwrap()
                                    .conflict_peers
                                    .insert(right);
                            }
                        }
                        let mut scheduler = GreedyScheduler::with_config(SchedulerConfig {
                            deferral_enabled,
                            merge_threshold,
                        });
                        for (callee, profile) in &profiles {
                            scheduler.set_profile(*callee, profile.clone());
                        }
                        let mut input = vec![
                            transaction(6, 1, 0, 1),
                            transaction(1, 1, 1, 2),
                            transaction(5, 2, 0, 2),
                            transaction(2, 2, 1, 3),
                            transaction(4, 3, 0, 3),
                            transaction(3, 3, 1, 1),
                        ];
                        input[1].fully_parallelizable = fully_parallelizable;
                        let plan = scheduler.schedule(input.clone());
                        assert_plan_invariants(&input, &profiles, &plan);
                    }
                }
            }
        }
    }
}

mod model_tests {
    use crate::scheduler::sender::SenderNonceOffset;
    use crate::scheduler::workload::{Generation, Job, JobSequence};

    fn transaction(sender: u8, nonce: u64, id: u64) -> Job {
        Job {
            id,
            sender: [sender; 20],
            nonce,
            callee: None,
            is_deferred: false,
            fully_parallelizable: true,
        }
    }

    fn offset(sender: u8, offset: u64) -> SenderNonceOffset {
        SenderNonceOffset {
            sender: [sender; 20],
            offset,
        }
    }

    #[test]
    fn finalized_offsets_count_transactions_once_per_sender_and_sequence() {
        let generation = Generation::new(vec![
            JobSequence::new(vec![
                transaction(2, 11, 5),
                transaction(1, 10, 1),
                transaction(1, 20, 3),
                transaction(2, 22, 7),
            ]),
            JobSequence::new(vec![transaction(2, 0, 4), transaction(1, 30, 2)]),
            JobSequence::new(vec![transaction(3, 2, 8), transaction(1, 40, 6)]),
        ]);

        assert_eq!(
            generation.sequences[0].nonce_offsets,
            vec![offset(1, 0), offset(2, 1)]
        );
        assert_eq!(
            generation.sequences[1].nonce_offsets,
            vec![offset(1, 2), offset(2, 0)]
        );
        assert_eq!(
            generation.sequences[2].nonce_offsets,
            vec![offset(1, 3), offset(3, 0)]
        );
    }
}

mod policy_tests {
    use crate::scheduler::job_resolver::{ExecutionMode, JobResolver};
    use crate::scheduler::{CalleeProfile, Job};
    use std::collections::{BTreeMap, BTreeSet};

    fn transaction(callee: u64, fully_parallelizable: bool) -> Job {
        Job {
            id: callee,
            sender: [0; 20],
            nonce: 0,
            callee: Some(callee),
            is_deferred: false,
            fully_parallelizable,
        }
    }

    #[test]
    fn unilateral_edges_apply_to_both_callees_even_without_a_peer_profile() {
        let profiles = BTreeMap::from([(
            1,
            CalleeProfile {
                conflict_peers: BTreeSet::from([2]),
                ..Default::default()
            },
        )]);
        let index = JobResolver::new(&profiles);
        let left = index.resolve(transaction(1, false));
        let right = index.resolve(transaction(2, false));

        assert_eq!(left.conflict_peer_count(), 1);
        assert_eq!(right.conflict_peer_count(), 1);
        assert!(!left.can_share_batch_with(&right));
        assert!(!right.can_share_batch_with(&left));
    }

    #[test]
    fn parallel_override_preserves_deferral_and_respects_peer_isolation() {
        let profiles = BTreeMap::from([
            (
                1,
                CalleeProfile {
                    conflict_peers: BTreeSet::from([2]),
                    sequential_only: true,
                    deferrable: true,
                },
            ),
            (
                3,
                CalleeProfile {
                    sequential_only: true,
                    ..Default::default()
                },
            ),
        ]);
        let index = JobResolver::new(&profiles);
        let parallel = index.resolve(transaction(1, true));
        let constrained = index.resolve(transaction(2, false));
        let isolated = index.resolve(transaction(3, false));

        assert_eq!(parallel.mode, ExecutionMode::FullyParallel);
        assert!(parallel.deferrable);
        assert!(parallel.can_share_batch_with(&constrained));
        assert!(constrained.can_share_batch_with(&parallel));
        assert!(!parallel.can_share_batch_with(&isolated));
        assert!(!isolated.can_share_batch_with(&parallel));
    }
}

mod queue_tests {
    use crate::scheduler::job_resolver::JobResolver;
    use crate::scheduler::sender_batcher::SenderBatcher;
    use crate::scheduler::{CalleeProfile, Job};
    use std::collections::{BTreeMap, BTreeSet};

    fn transaction(id: u64, sender: u8, nonce: u64, callee: u64) -> Job {
        Job {
            id,
            sender: [sender; 20],
            nonce,
            callee: Some(callee),
            is_deferred: false,
            fully_parallelizable: false,
        }
    }

    #[test]
    fn repeatedly_scans_heads_without_reordering_a_sender() {
        let profiles = BTreeMap::new();
        let conflicts = JobResolver::new(&profiles);
        let transactions = [
            transaction(10, 1, 2, 0),
            transaction(30, 1, 0, 0),
            transaction(20, 1, 1, 0),
            transaction(5, 2, 0, 0),
            transaction(50, 2, 1, 0),
        ];
        let mut queues = SenderBatcher::new(transactions.map(|tx| conflicts.resolve(tx)));

        let ids = queues
            .next_batch()
            .unwrap()
            .into_iter()
            .map(|tx| tx.job.id)
            .collect::<Vec<_>>();

        assert_eq!(ids, [5, 50, 30, 20, 10]);
        assert!(queues.next_batch().is_none());
    }

    #[test]
    fn a_conflicting_head_blocks_later_transactions_from_that_sender() {
        let profiles = BTreeMap::from([(
            1,
            CalleeProfile {
                conflict_peers: BTreeSet::from([2]),
                ..Default::default()
            },
        )]);
        let conflicts = JobResolver::new(&profiles);
        let transactions = [
            transaction(1, 1, 0, 1),
            transaction(2, 2, 0, 2),
            transaction(3, 2, 1, 3),
        ];
        let mut queues = SenderBatcher::new(transactions.map(|tx| conflicts.resolve(tx)));

        let first = queues.next_batch().unwrap();
        let second = queues.next_batch().unwrap();

        assert_eq!(first.len(), 1);
        assert_eq!(first[0].job.id, 1);
        assert_eq!(
            second.into_iter().map(|tx| tx.job.id).collect::<Vec<_>>(),
            [2, 3]
        );
        assert!(queues.next_batch().is_none());
    }
}
