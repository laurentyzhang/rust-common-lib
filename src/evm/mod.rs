//! Arcology-specific hooks used by an EVM adapter.
//!
//! This module deliberately keeps the hook contract independent from a specific
//! REVM release. The Reth/REVM integration only needs to translate its call
//! frame into [`CallHookInput`] and translate [`CallHookOutcome`] back.

#[doc(hidden)]
pub mod backend;
mod call_router;
mod executor;
mod executor_factory;
mod factory;
mod revm_adapter;
mod transaction;

pub use backend::ArcologyBlockExecutorFactory;
pub use call_router::{
    ArcologyApi, CallHook, CallHookError, CallHookInput, CallHookOutcome, CallHookOutput,
    CallHookRouter, CallScheme, ClosureCallHook, ExitStatus, RegisterHookError, arcology_address,
};
pub use executor::{ArcologyBlockExecutor, ArcologyParaExecutor};
pub use executor_factory::ArcologyExecutorFactory;
pub use factory::{ArcologyEvm, ArcologyEvmFactory, PrecompileCollisionError};
pub use revm_adapter::dispatch_revm_call;
pub use transaction::ArcologyTx;
