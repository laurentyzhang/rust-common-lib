use super::*;
use alloy_evm::revm::{
    context_interface::result::{ExecutionResult, Output, ResultGas, SuccessReason},
    state::{AccountInfo, EvmStorageSlot},
};
use alloy_primitives::Bytes;

#[test]
fn zero_deletes_only_ordinary_evm_slots() {
    use crate::{namespace::Namespace, planner::meta::MetaFieldId as MetaProperty};

    let owner = Address::with_last_byte(1);
    let slots = [
        (U256::ZERO, true),
        (
            ScalarKeyCodec::encode(Namespace::U256, Property::PosDelta as u8, Address::ZERO),
            false,
        ),
        (
            ScalarKeyCodec::encode(Namespace::I256, Property::NegDelta as u8, Address::ZERO),
            false,
        ),
        (MetaProperty::DeferredGas.slot(owner, 0, 0), false),
    ];
    let mut account = Account::from(AccountInfo::default().with_nonce(1));
    for &(slot, _) in &slots {
        account.storage.insert(
            slot,
            EvmStorageSlot::new_changed(U256::from(2), U256::ZERO, Default::default()),
        );
    }
    let job = JobResult::<()>::new(
        ID::new(0, 0),
        Ok(ResultAndState::new(
            ExecutionResult::Success {
                reason: SuccessReason::Stop,
                gas: ResultGas::default(),
                logs: Vec::new(),
                output: Output::Call(Bytes::new()),
            },
            EvmState::from_iter([(owner, account)]),
        )),
        None,
        Vec::new(),
    );
    let records = job.to_records(job.id);
    for (slot, deleted) in slots {
        let record = records
            .iter()
            .find(|record| record.key == Key::Storage(owner, slot))
            .unwrap();
        assert_eq!(
            matches!(record.data.current(), Status::Tag(Tag::Deleted)),
            deleted
        );
        assert_eq!(record.data.operations.removes.count(), u32::from(deleted));
        assert_eq!(record.data.operations.deltas.count(), u32::from(!deleted));
    }
}

#[test]
fn rejected_result_is_built_on_demand_from_original_accounts() {
    let sender = Address::with_last_byte(1);
    let recipient = Address::with_last_byte(2);
    let mut sender_account = Account::from(AccountInfo {
        balance: U256::from(100),
        ..Default::default()
    });
    sender_account.info.nonce = 1;
    sender_account.info.balance = U256::from(50);
    sender_account.storage.insert(
        U256::ZERO,
        EvmStorageSlot::new_changed(U256::ZERO, U256::from(1), Default::default()),
    );
    let recipient_account = Account::from(AccountInfo {
        balance: U256::from(20),
        ..Default::default()
    });
    let result = ExecutionResult::Success {
        reason: SuccessReason::Stop,
        gas: ResultGas::default().with_total_gas_spent(21_000),
        logs: Vec::new(),
        output: Output::Call(Bytes::new()),
    };
    let mut job = JobResult::<()>::new(
        ID::new(0, 0),
        Ok(ResultAndState::new(
            result,
            EvmState::from_iter([(sender, sender_account), (recipient, recipient_account)]),
        )),
        None,
        Vec::new(),
    );
    job.sender = sender;
    job.fee = U256::from(10);
    job.fee_recipients = vec![(sender, U256::from(2)), (recipient, U256::from(8))];
    assert!(
        matches!(job.result_and_state(), Err(error) if error.to_string().contains("pending verdict"))
    );
    job.verdict.decision = CommitDecision::Accepted;
    assert_eq!(
        job.result_and_state().as_ref().unwrap().state[&sender]
            .info
            .balance,
        U256::from(50)
    );

    job.verdict.decision = CommitDecision::Rejected;
    let executed = job.result_and_state().as_ref().unwrap();
    assert_eq!(executed.result.tx_gas_used(), 21_000);
    assert_eq!(executed.state[&sender].info.balance, U256::from(92));
    assert_eq!(executed.state[&sender].info.nonce, 1);
    assert!(executed.state[&sender].storage.is_empty());
    assert_eq!(executed.state[&recipient].info.balance, U256::from(28));

    job.fee = U256::from(11);
    assert_eq!(
        job.result_and_state().as_ref().unwrap().state[&sender]
            .info
            .balance,
        U256::from(91)
    );
    job.fee = U256::from(101);
    assert!(job.result_and_state().is_err());
    job.verdict.decision = CommitDecision::Accepted;
    assert_eq!(
        job.result_and_state().as_ref().unwrap().state[&sender]
            .info
            .balance,
        U256::from(50)
    );
}

#[test]
fn rejected_execution_error_is_preserved() {
    let mut job = JobResult::<()>::new(
        ID::new(0, 0),
        Err(BlockExecutionError::msg("execution failed")),
        None,
        Vec::new(),
    );
    job.verdict.decision = CommitDecision::Rejected;
    assert_eq!(
        job.result_and_state().as_ref().unwrap_err().to_string(),
        "execution failed"
    );
}
