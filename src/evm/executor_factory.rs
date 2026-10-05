//! Arcology block-executor construction.

use super::{
    ArcologyEvmFactory,
    backend::{ArcologyBlockExecutorFactory, ArcologyBlockFactory},
    executor::{ArcologyBlockExecutor, ArcologyParaExecutor},
};
use alloy_evm::{
    Evm, EvmEnv, EvmFactory,
    block::{BlockExecutorFactory, StateDB},
    revm::{
        Inspector,
        database::{CacheDB, EmptyDB},
        inspector::NoOpInspector,
    },
};

/// Creates block executors backed by [`ArcologyEvmFactory`].
#[derive(Clone, Debug)]
pub struct ArcologyExecutorFactory<R, Spec> {
    base: ArcologyBlockExecutorFactory<R, Spec>,
    evm_factory: ArcologyEvmFactory,
}

impl<R, Spec> ArcologyExecutorFactory<R, Spec> {
    pub fn new(receipt_builder: R, spec: Spec, evm_factory: ArcologyEvmFactory) -> Self {
        Self {
            base: ArcologyBlockExecutorFactory::new(receipt_builder, spec, evm_factory.clone()),
            evm_factory,
        }
    }
}

impl<R, Spec> ArcologyExecutorFactory<R, Spec>
where
    ArcologyBlockExecutorFactory<R, Spec>: ArcologyBlockFactory,
    R: 'static,
    Spec: 'static,
{
    /// Builds Arcology EVM workers from one mock-backed EVM template.
    ///
    /// Each worker receives an independent clone of the template database and the same EVM
    /// environment. This is the mock-database entry point for developing the parallel executor.
    pub fn create_para_executor<'a>(
        &'a self,
        template: <<Self as BlockExecutorFactory>::EvmFactory as EvmFactory>::Evm<
            CacheDB<EmptyDB>,
            NoOpInspector,
        >,
        ctx: <Self as BlockExecutorFactory>::ExecutionCtx<'a>,
        worker_count: usize,
    ) -> ArcologyParaExecutor<<Self as BlockExecutorFactory>::ExecutionCtx<'a>> {
        let db = template.db().clone();
        let env = EvmEnv::new(template.cfg_env().clone(), template.block().clone());
        let workers = (0..worker_count)
            .map(|_| self.evm_factory.create_evm(db.clone(), env.clone()))
            .collect();
        drop(template);

        ArcologyParaExecutor::new(workers, ctx)
    }
}

impl<R, Spec> BlockExecutorFactory for ArcologyExecutorFactory<R, Spec>
where
    ArcologyBlockExecutorFactory<R, Spec>: ArcologyBlockFactory,
    R: 'static,
    Spec: 'static,
{
    type EvmFactory = <ArcologyBlockExecutorFactory<R, Spec> as BlockExecutorFactory>::EvmFactory;
    type ExecutionCtx<'a> =
        <ArcologyBlockExecutorFactory<R, Spec> as BlockExecutorFactory>::ExecutionCtx<'a>;
    type Transaction = <ArcologyBlockExecutorFactory<R, Spec> as BlockExecutorFactory>::Transaction;
    type Receipt = <ArcologyBlockExecutorFactory<R, Spec> as BlockExecutorFactory>::Receipt;
    type TxExecutionResult =
        <ArcologyBlockExecutorFactory<R, Spec> as BlockExecutorFactory>::TxExecutionResult;
    type Executor<'a, DB: StateDB, I: Inspector<<Self::EvmFactory as EvmFactory>::Context<DB>>> =
        ArcologyBlockExecutor<
            <ArcologyBlockExecutorFactory<R, Spec> as BlockExecutorFactory>::Executor<'a, DB, I>,
        >;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.base.evm_factory()
    }

    fn create_executor<'a, DB, I>(
        &'a self,
        evm: <Self::EvmFactory as EvmFactory>::Evm<DB, I>,
        ctx: Self::ExecutionCtx<'a>,
    ) -> Self::Executor<'a, DB, I>
    where
        DB: StateDB,
        I: Inspector<<Self::EvmFactory as EvmFactory>::Context<DB>>,
    {
        ArcologyBlockExecutor::new(self.base.create_executor(evm, ctx))
    }
}

#[cfg(test)]
#[path = "executor_tests.rs"]
mod tests;
