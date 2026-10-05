//! OP-backed EVM construction with Arcology call hooks installed as stateful precompiles.
//!
//! The OP factory remains responsible for constructing the EVM and selecting its native
//! precompiles. Arcology only validates its addresses and extends the completed OP map.

use super::{
    CallHookRouter,
    backend::{
        BaseEvm, BaseFactory, PostExecEvm, PostExecEvmFactoryHooks, PostExecExecutedTx,
        PostExecTxContext,
    },
    dispatch_revm_call,
    transaction::ArcologyTx,
};
use alloy_evm::{
    Database, Evm, EvmEnv, EvmFactory,
    precompiles::{DynPrecompile, Precompile, PrecompilesMap},
    revm::{
        Inspector,
        context::{CfgEnv, DBErrorMarker},
        context_interface::result::ResultAndState,
        inspector::NoOpInspector,
        precompile::{PrecompileId, Precompiles},
    },
};

use alloy_primitives::{Address, Bytes};
use std::{error::Error, fmt};

/// Returned when an Arcology handler targets an address already occupied by
/// the active Ethereum precompile set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompileCollisionError {
    pub address: Address,
    pub existing: PrecompileId,
}

impl fmt::Display for PrecompileCollisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Arcology precompile address {:?} collides with existing precompile {}",
            self.address,
            self.existing.name()
        )
    }
}

impl Error for PrecompileCollisionError {}

/// Arcology EVM wrapper.
///
/// Its OP EVM is deliberately private so callers only depend on the
/// Arcology surface.
#[allow(missing_debug_implementations)]
pub struct ArcologyEvm<DB: Database, I> {
    // Keep the backend implementation private while exposing Alloy's standard Evm interface.
    base_evm: BaseEvm<DB, I>,
}

/// Delegates the Alloy EVM interface to the fully configured OP EVM.
impl<DB, I> Evm for ArcologyEvm<DB, I>
where
    DB: Database,
    I: Inspector<<BaseFactory as EvmFactory>::Context<DB>>,
{
    type DB = DB;
    type Tx = ArcologyTx;
    type Error = <BaseFactory as EvmFactory>::Error<DB::Error>;
    type HaltReason = <BaseFactory as EvmFactory>::HaltReason;
    type Spec = <BaseFactory as EvmFactory>::Spec;
    type BlockEnv = <BaseFactory as EvmFactory>::BlockEnv;
    type Precompiles = PrecompilesMap;
    type Inspector = I;

    fn block(&self) -> &Self::BlockEnv {
        self.base_evm.block()
    }

    fn cfg_env(&self) -> &CfgEnv<Self::Spec> {
        self.base_evm.cfg_env()
    }

    fn chain_id(&self) -> u64 {
        self.base_evm.chain_id()
    }

    fn transact_raw(
        &mut self,
        tx: Self::Tx,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.base_evm.transact_raw(tx)
    }

    fn transact_system_call(
        &mut self,
        caller: Address,
        contract: Address,
        data: Bytes,
    ) -> Result<ResultAndState<Self::HaltReason>, Self::Error> {
        self.base_evm.transact_system_call(caller, contract, data)
    }

    fn finish(self) -> (Self::DB, EvmEnv<Self::Spec, Self::BlockEnv>) {
        self.base_evm.finish()
    }

    fn set_inspector_enabled(&mut self, enabled: bool) {
        self.base_evm.set_inspector_enabled(enabled);
    }

    fn components(&self) -> (&Self::DB, &Self::Inspector, &Self::Precompiles) {
        self.base_evm.components()
    }

    fn components_mut(&mut self) -> (&mut Self::DB, &mut Self::Inspector, &mut Self::Precompiles) {
        self.base_evm.components_mut()
    }
}

/// Preserves OP's block-scoped post-execution accounting through the wrapper.
impl<DB, I> PostExecEvm for ArcologyEvm<DB, I>
where
    DB: Database,
    I: Inspector<<BaseFactory as EvmFactory>::Context<DB>>,
{
    type Snapshot = <BaseEvm<DB, I> as PostExecEvm>::Snapshot;

    fn begin_post_exec_tx(&mut self, ctx: PostExecTxContext) {
        self.base_evm.begin_post_exec_tx(ctx);
    }

    fn take_last_post_exec_tx_result(&mut self) -> PostExecExecutedTx {
        self.base_evm.take_last_post_exec_tx_result()
    }

    fn refund_snapshot(&self) -> Self::Snapshot {
        self.base_evm.refund_snapshot()
    }

    fn seed_refund_snapshot(&mut self, state: Self::Snapshot) {
        self.base_evm.seed_refund_snapshot(state);
    }
}

/// Reth/Alloy factory that creates a complete OP EVM first and then installs
/// the registered Arcology call hooks into that instance.
#[derive(Clone, Debug, Default)]
pub struct ArcologyEvmFactory {
    base_evm_factory: BaseFactory,
    router: CallHookRouter,
}

impl ArcologyEvmFactory {
    /// Creates a factory after checking every registered Arcology address
    /// against the complete OP precompile set.
    pub fn try_new(router: CallHookRouter) -> Result<Self, PrecompileCollisionError> {
        let op_precompiles = PrecompilesMap::from_static(Precompiles::latest());
        Self::ensure_no_collisions(&router, &op_precompiles)?;
        Ok(Self {
            base_evm_factory: BaseFactory::default(),
            router,
        })
    }

    pub fn router(&self) -> &CallHookRouter {
        &self.router
    }

    fn ensure_no_collisions(
        router: &CallHookRouter,
        precompiles: &PrecompilesMap,
    ) -> Result<(), PrecompileCollisionError> {
        // Never let Arcology replace an already registered OP/Ethereum precompile.
        for address in router.registered_addresses() {
            if let Some(existing) = precompiles.get(&address) {
                return Err(PrecompileCollisionError {
                    address,
                    existing: existing.precompile_id().clone(),
                });
            }
        }
        Ok(())
    }

    fn try_inject_precompiles(
        &self,
        precompiles: &mut PrecompilesMap,
    ) -> Result<(), PrecompileCollisionError> {
        // This receives the map after the backend factory has populated it.
        Self::ensure_no_collisions(&self.router, &precompiles)?;

        // Each Arcology address becomes a stateful REVM precompile that routes into CallHookRouter.
        let hooks = self
            .router
            .registered_addresses()
            .into_iter()
            .map(|address| {
                let router = self.router.clone();
                let id = PrecompileId::custom(format!("arcology-{address:?}"));
                let hook = DynPrecompile::new_stateful(id, move |input| {
                    dispatch_revm_call(&router, input)
                });
                (address, hook)
            });

        // Extension is safe because every target address was checked immediately above.
        precompiles.extend_precompiles(hooks);
        Ok(())
    }

    fn inject_into<DB, I>(&self, base_evm: &mut BaseEvm<DB, I>)
    where
        DB: Database,
        I: Inspector<<BaseFactory as EvmFactory>::Context<DB>>,
    {
        // try_new establishes this invariant; checking again protects the actual per-EVM map.
        self.try_inject_precompiles(base_evm.precompiles_mut())
            .expect("Arcology precompile address collides with an OP precompile");
    }
}

impl EvmFactory for ArcologyEvmFactory {
    type Evm<DB: Database, I: Inspector<Self::Context<DB>>> = ArcologyEvm<DB, I>;
    type Context<DB: Database> = <BaseFactory as EvmFactory>::Context<DB>;
    type Tx = ArcologyTx;
    type Error<DBError: DBErrorMarker> = <BaseFactory as EvmFactory>::Error<DBError>;
    type HaltReason = <BaseFactory as EvmFactory>::HaltReason;
    type Spec = <BaseFactory as EvmFactory>::Spec;
    type BlockEnv = <BaseFactory as EvmFactory>::BlockEnv;
    type Precompiles = PrecompilesMap;

    fn create_evm<DB: Database>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec, Self::BlockEnv>,
    ) -> Self::Evm<DB, NoOpInspector> {
        // 1. Let OP construct the context, handlers, and native precompiles.
        let mut base_evm = self.base_evm_factory.create_evm(db, input);

        // 2. Add Arcology handlers without replacing anything OP registered.
        self.inject_into(&mut base_evm);

        // 3. Hide the OP EVM behind Arcology's wrapper.
        ArcologyEvm { base_evm }
    }

    fn create_evm_with_inspector<DB: Database, I: Inspector<Self::Context<DB>>>(
        &self,
        db: DB,
        input: EvmEnv<Self::Spec, Self::BlockEnv>,
        inspector: I,
    ) -> Self::Evm<DB, I> {
        // The inspected path follows the same OP-first construction order.
        let mut base_evm = self
            .base_evm_factory
            .create_evm_with_inspector(db, input, inspector);
        self.inject_into(&mut base_evm);
        ArcologyEvm { base_evm }
    }
}

/// Exposes the wrapped OP post-execution hooks to OP's block-executor adapter.
impl PostExecEvmFactoryHooks for ArcologyEvmFactory {
    type Snapshot = <BaseFactory as PostExecEvmFactoryHooks>::Snapshot;

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

#[cfg(test)]
#[path = "factory_tests.rs"]
mod tests;
