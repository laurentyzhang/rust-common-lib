use super::*;
use crate::evm::{
    ArcologyApi, CallHookInput, CallHookOutput, CallHookRouter, ClosureCallHook,
};
use alloy_evm::{
    Evm, EvmEnv,
    revm::database::{CacheDB, EmptyDB, State},
};
use alloy_op_evm::{OpBlockExecutionCtx, block::OpAlloyReceiptBuilder};
use alloy_op_hardforks::OpChainHardforks;
use alloy_primitives::Bytes;

#[test]
fn creates_executor_with_arcology_precompiles() {
    let mut router = CallHookRouter::new();
    router
        .register(
            ArcologyApi::Runtime,
            ClosureCallHook::new(|_: &CallHookInput<'_>| {
                Ok(CallHookOutput::returned(1, Bytes::new()))
            }),
        )
        .unwrap();

    let factory = ArcologyExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(router).unwrap(),
    );
    let mut db = State::builder()
        .with_database(CacheDB::<EmptyDB>::default())
        .build();
    let evm = factory.evm_factory().create_evm(&mut db, EvmEnv::default());

    assert!(
        evm.precompiles()
            .get(&ArcologyApi::Runtime.address())
            .is_some()
    );

    let _executor = factory.create_executor(evm, OpBlockExecutionCtx::default());
}

#[test]
fn creates_parallel_workers_from_one_mock_evm() {
    let mut router = CallHookRouter::new();
    router
        .register(
            ArcologyApi::Runtime,
            ClosureCallHook::new(|_: &CallHookInput<'_>| {
                Ok(CallHookOutput::returned(1, Bytes::new()))
            }),
        )
        .unwrap();
    let factory = ArcologyExecutorFactory::new(
        OpAlloyReceiptBuilder::default(),
        OpChainHardforks::op_mainnet(),
        ArcologyEvmFactory::try_new(router).unwrap(),
    );
    let evm = factory
        .evm_factory()
        .create_evm(CacheDB::<EmptyDB>::default(), EvmEnv::default());

    let parallel = factory.create_para_executor(evm, OpBlockExecutionCtx::default(), 3);

    assert_eq!(parallel.workers().len(), 3);
    for worker in parallel.workers() {
        assert!(
            worker
                .precompiles()
                .get(&ArcologyApi::Runtime.address())
                .is_some()
        );
    }
}
