//! Arcology-specific hooks used by an EVM adapter.
//!
//! Call metadata is exposed through [`CallInput`]; stateful hooks also receive
//! Alloy's journal-backed `EvmInternals` for transactional state access.

mod error;
mod executor;
mod hooks;
mod op;
mod outcome;
mod slot;
mod state;
mod tracking;

#[cfg(test)]
mod tests;

pub use error::{AccountDecodeError, CallHookError, PrecompileCollisionError, RegisterHookError};
pub use executor::{ArcologyParaBlockExecutor, ParaBlockConfig};
pub use hooks::{
    ApiAddress, CallHandler, CallInput, CallOutput, CallRouter, CallScheme, ExitStatus,
    U256Handler, dispatch_precompile,
};
#[doc(hidden)]
pub use op::backend;
pub use op::{ArcologyBlockExecutorFactory, ArcologyEvm, ArcologyEvmFactory, ArcologyTx};
pub use outcome::{JobResult, TransactionOutcome};
pub use slot::KeyCodec;
pub(in crate::execution::evm) use slot::{API_OFFSET, FIELD_OFFSET, Field, ScalarSlots};
pub use state::key;
pub use state::{AccountInfoCodec, EvmStateAccumulator, EvmStateCache, MergeableEvmState};
pub use tracking::{ReadRecorder, StateReads, recorder};
