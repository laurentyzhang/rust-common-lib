//! Arcology-specific hooks used by an EVM adapter.
//!
//! This module deliberately keeps the hook contract independent from a specific
//! REVM release. The Reth/REVM integration only needs to translate its call
//! frame into [`CallHookInput`] and translate [`CallHookOutcome`] back.

mod account_info;
#[doc(hidden)]
pub mod backend;
mod block_executor;
mod call_router;
mod dispatch;
mod factory;
mod statedb;
mod transaction;

pub use account_info::{AccountDecodeError, AccountInfoCodec};
pub use backend::ArcologyBlockExecutorFactory;
pub use block_executor::{ArcologyParaBlockExecutor, ParaBlockConfig};
pub use call_router::{
    ArcologyApi, CallHook, CallHookError, CallHookInput, CallHookOutcome, CallHookOutput,
    CallHookRouter, CallScheme, ClosureCallHook, ExitStatus, RegisterHookError, arcology_address,
};
pub use dispatch::dispatch_precompile;
pub use factory::{ArcologyEvm, ArcologyEvmFactory, PrecompileCollisionError};
pub use statedb::{Key, KeyCodec, StateDB};
pub use transaction::ArcologyTx;
