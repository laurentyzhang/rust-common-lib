//! Private bridge between Arcology's public EVM types and the OP implementation.

use super::{factory::ArcologyEvmFactory, transaction::ArcologyTx};
use alloy_evm::{
    Database, Evm, EvmEnv, EvmFactory,
    block::{BlockExecutorFactory, StateDB},
    precompiles::PrecompilesMap,
    revm::{
        Inspector,
        context::{CfgEnv, DBErrorMarker},
        context_interface::result::ResultAndState,
        inspector::NoOpInspector,
    },
};
use alloy_op_evm::{
    OpBlockExecutorFactory, OpEvm, OpEvmContext, OpEvmFactory, post_exec::PostExecEvmFactoryAdapter,
};
use alloy_primitives::{Address, Bytes};

pub(super) use alloy_op_evm::post_exec::{
    PostExecEvm, PostExecEvmFactoryHooks, PostExecExecutedTx, PostExecTxContext,
};

#[doc(hidden)]
pub trait ArcologyBlockFactory {
    type Factory: BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>;

    fn inner_factory(&self) -> &Self::Factory;
}

impl<R, Spec> ArcologyBlockFactory for ArcologyBlockExecutorFactory<R, Spec>
where
    OpBlockExecutorFactory<R, Spec, PostExecEvmFactoryAdapter<ArcologyEvmFactory>>:
        BlockExecutorFactory<EvmFactory = PostExecEvmFactoryAdapter<ArcologyEvmFactory>>,
{
    type Factory = OpBlockExecutorFactory<R, Spec, PostExecEvmFactoryAdapter<ArcologyEvmFactory>>;

    fn inner_factory(&self) -> &Self::Factory {
        &self.factory
    }
}

/// Internal EVM wrapper containing every concrete OP EVM type.
#[allow(missing_debug_implementations)]
#[doc(hidden)]
pub struct BaseEvm<DB: Database, I> {
    op_evm: OpEvm<DB, I, PrecompilesMap, ArcologyTx>,
}

impl<DB, I> Evm for BaseEvm<DB, I>
where
    DB: Database,
    I: Inspector<OpEvmContext<DB>>,
{
    type DB = DB;
    type Tx = ArcologyTx;
    type Error = <OpEvmFactory<ArcologyTx> as EvmFactory>::Error<DB::Error>;
    type HaltReason = <OpEvmFactory<ArcologyTx> as EvmFactory>::HaltReason;
    type Spec = <OpEvmFactory<ArcologyTx> as EvmFactory>::Spec;
    type BlockEnv = <OpEvmFactory<ArcologyTx> as EvmFactory>::BlockEnv;
    type Precompiles = PrecompilesMap;
    type Inspector = I;

    fn block(&self) -> &Self::BlockEnv {
        self.op_evm.block()
    }

    fn cfg_env(&self) -> &CfgEnv<Self::Spec> {
        self.op_evm.cfg_env()
    }

    fn chain_id(&self) -> u64 {
        self.op_evm.chain_id()
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.op_evm.transact_raw(tx)
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.op_evm.transact_system_call(caller, contract, data)
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec, Self::BlockEnv>) {
        self.op_evm.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.op_evm.set_inspector_enabled(enabled);
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        self.op_evm.components()
    }

    fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        self.op_evm.components_mut()
    }
}

impl<DB, I> PostExecEvm for BaseEvm<DB, I>
where
    DB: Database,
    I: Inspector<OpEvmContext<DB>>,
{
    type Snapshot = <OpEvm<DB, I, PrecompilesMap, ArcologyTx> as PostExecEvm>::Snapshot;

    fn begin_post_exec_tx(&mut self, ctx: PostExecTxContext) {
        self.op_evm.begin_post_exec_tx(ctx);
    }

    fn take_last_post_exec_tx_result(&mut self) -> PostExecExecutedTx {
        self.op_evm.take_last_post_exec_tx_result()
    }

    fn refund_snapshot(&self) -> Self::Snapshot {
        self.op_evm.refund_snapshot()
    }

    fn seed_refund_snapshot(&mut self, state: Self::Snapshot) {
        self.op_evm.seed_refund_snapshot(state);
    }
}

/// Internal factory wrapper that owns the concrete OP factory.
#[derive(Clone, Debug, Default)]
#[doc(hidden)]
pub struct BaseFactory {
    op_evm_factory: OpEvmFactory<ArcologyTx>,
}

impl EvmFactory for BaseFactory {
    type Evm<DB: Database, I: Inspector<Self::Context<DB>>> = BaseEvm<DB, I>;
    type Context<DB: Database> = OpEvmContext<DB>;
    type Tx = ArcologyTx;
    type Error<DBError: DBErrorMarker> = <OpEvmFactory<ArcologyTx> as EvmFactory>::Error<DBError>;
    type HaltReason = <OpEvmFactory<ArcologyTx> as EvmFactory>::HaltReason;
    type Spec = <OpEvmFactory<ArcologyTx> as EvmFactory>::Spec;
    type BlockEnv = <OpEvmFactory<ArcologyTx> as EvmFactory>::BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec, Self::BlockEnv>,
    ) -> Self::Evm<DB, NoOpInspector> {
        BaseEvm {
            op_evm: self.op_evm_factory.create_evm(db, input),
        }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec, Self::BlockEnv>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        BaseEvm {
            op_evm: self
                .op_evm_factory
                .create_evm_with_inspector(db, input, inspector),
        }
    }
}

impl PostExecEvmFactoryHooks for BaseFactory {
    type Snapshot = <OpEvmFactory<ArcologyTx> as PostExecEvmFactoryHooks>::Snapshot;

    fn begin_post_exec_tx<DB, I>(evm: &mut Self::Evm<DB, I>, ctx: PostExecTxContext)
    where
        DB: Database,
        I: Inspector<Self::Context<DB>>,
    {
        evm.begin_post_exec_tx(ctx);
    }

    fn take_last_post_exec_tx_result<DB, I>(evm: &mut Self::Evm<DB, I>) -> PostExecExecutedTx
    where
        DB: Database,
        I: Inspector<Self::Context<DB>>,
    {
        evm.take_last_post_exec_tx_result()
    }

    fn refund_snapshot<DB, I>(evm: &Self::Evm<DB, I>) -> Self::Snapshot
    where
        DB: Database,
        I: Inspector<Self::Context<DB>>,
    {
        evm.refund_snapshot()
    }

    fn seed_refund_snapshot<DB, I>(evm: &mut Self::Evm<DB, I>, state: Self::Snapshot)
    where
        DB: Database,
        I: Inspector<Self::Context<DB>>,
    {
        evm.seed_refund_snapshot(state);
    }
}

/// Arcology wrapper around the chain's block-executor factory.
///
/// The wrapped factory and all chain-specific concrete types remain private.
#[derive(Clone, Debug)]
pub struct ArcologyBlockExecutorFactory<R, Spec> {
    factory: OpBlockExecutorFactory<R, Spec, PostExecEvmFactoryAdapter<ArcologyEvmFactory>>,
}

impl<R, Spec> ArcologyBlockExecutorFactory<R, Spec> {
    pub fn new(receipt_builder: R, spec: Spec, evm_factory: ArcologyEvmFactory) -> Self {
        Self {
            factory: OpBlockExecutorFactory::new(
                receipt_builder,
                spec,
                PostExecEvmFactoryAdapter::new(evm_factory),
            ),
        }
    }
}

impl<R, Spec> BlockExecutorFactory for ArcologyBlockExecutorFactory<R, Spec>
where
    Self: ArcologyBlockFactory,
    R: 'static,
    Spec: 'static,
{
    type EvmFactory = <<Self as ArcologyBlockFactory>::Factory as BlockExecutorFactory>::EvmFactory;
    type ExecutionCtx<'a> =
        <<Self as ArcologyBlockFactory>::Factory as BlockExecutorFactory>::ExecutionCtx<'a>;
    type Transaction =
        <<Self as ArcologyBlockFactory>::Factory as BlockExecutorFactory>::Transaction;
    type Receipt = <<Self as ArcologyBlockFactory>::Factory as BlockExecutorFactory>::Receipt;
    type TxExecutionResult =
        <<Self as ArcologyBlockFactory>::Factory as BlockExecutorFactory>::TxExecutionResult;
    type Executor<'a, DB: StateDB, I: Inspector<<Self::EvmFactory as EvmFactory>::Context<DB>>> =
        <<Self as ArcologyBlockFactory>::Factory as BlockExecutorFactory>::Executor<'a, DB, I>;

    fn evm_factory(&self) -> &Self::EvmFactory {
        self.inner_factory().evm_factory()
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
        self.inner_factory().create_executor(evm, ctx)
    }
}
