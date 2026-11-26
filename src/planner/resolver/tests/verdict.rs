use super::*;

#[test]
fn pending_is_neither_accepted_nor_rejected() {
    let verdicts = [
        CommitVerdict::<()> {
            id: ID::new(0, 0),
            decision: CommitDecision::Pending,
            conflicts: Vec::new(),
        },
        CommitVerdict {
            id: ID::new(0, 1),
            decision: CommitDecision::Accepted,
            conflicts: Vec::new(),
        },
        CommitVerdict {
            id: ID::new(0, 2),
            decision: CommitDecision::Rejected,
            conflicts: Vec::new(),
        },
    ];
    assert_eq!(accepted_ids(&verdicts), vec![verdicts[1].id]);
    assert_eq!(rejected_ids(&verdicts), vec![verdicts[2].id]);
}
