//! Arcology's parallel execution worker set.

use alloy_evm::{
    block::{BlockExecutionError, BlockExecutionResult, BlockExecutor, ExecutableTx, GasOutput},
    revm::{
        database::{CacheDB, EmptyDB},
        inspector::NoOpInspector,
    },
};

/// Arcology's block executor. Its block method currently delegates to Alloy's default loop.
pub struct ArcologyBlockExecutor<E> {
    inner: E,
}

impl<E> ArcologyBlockExecutor<E> {
    pub(super) fn new(inner: E) -> Self {
        Self { inner }
    }
}

impl<E: BlockExecutor> BlockExecutor for ArcologyBlockExecutor<E> {
    type Transaction = E::Transaction;
    type Receipt = E::Receipt;
    type Evm = E::Evm;
    type Result = E::Result;

    fn apply_pre_execution_changes(&mut self) -> Result<(), BlockExecutionError> {
        self.inner.apply_pre_execution_changes()
    }

    fn execute_transaction_without_commit(
        &mut self,
        tx: impl ExecutableTx<Self>,
    ) -> Result<Self::Result, BlockExecutionError> {
        self.inner.execute_transaction_without_commit(tx)
    }

    fn commit_transaction(&mut self, output: Self::Result) -> GasOutput {
        self.inner.commit_transaction(output)
    }

    fn finish(
        self,
    ) -> Result<(Self::Evm, BlockExecutionResult<Self::Receipt>), BlockExecutionError> {
        self.inner.finish()
    }

    fn evm_mut(&mut self) -> &mut Self::Evm {
        self.inner.evm_mut()
    }

    fn evm(&self) -> &Self::Evm {
        self.inner.evm()
    }

    fn receipts(&self) -> &[Self::Receipt] {
        self.inner.receipts()
    }

    fn execute_block(
        self,
        transactions: impl IntoIterator<Item = impl ExecutableTx<Self>>,
    ) -> Result<BlockExecutionResult<Self::Receipt>, BlockExecutionError>
    where
        Self: Sized,
    {
        <E as BlockExecutor>::execute_block(self.inner, transactions)
    }
}

/// EVM workers and block context for Arcology's parallel execution path.
pub struct ArcologyParaExecutor<Ctx> {
    workers: Vec<super::ArcologyEvm<CacheDB<EmptyDB>, NoOpInspector>>,
    execution_context: Ctx,
}

impl<Ctx> ArcologyParaExecutor<Ctx> {
    pub(super) fn new(
        workers: Vec<super::ArcologyEvm<CacheDB<EmptyDB>, NoOpInspector>>,
        execution_context: Ctx,
    ) -> Self {
        Self {
            workers,
            execution_context,
        }
    }

    pub fn workers(&self) -> &[super::ArcologyEvm<CacheDB<EmptyDB>, NoOpInspector>] {
        &self.workers
    }

    pub fn workers_mut(&mut self) -> &mut [super::ArcologyEvm<CacheDB<EmptyDB>, NoOpInspector>] {
        &mut self.workers
    }

    pub fn execution_context(&self) -> &Ctx {
        &self.execution_context
    }

    pub fn into_parts(
        self,
    ) -> (
        Vec<super::ArcologyEvm<CacheDB<EmptyDB>, NoOpInspector>>,
        Ctx,
    ) {
        (self.workers, self.execution_context)
    }
}
