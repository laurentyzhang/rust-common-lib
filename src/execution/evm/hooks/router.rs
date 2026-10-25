use alloy_evm::{
    EvmInternals,
    precompiles::PrecompileInput,
    revm::precompile::{PrecompileError, PrecompileHalt, PrecompileOutput, PrecompileResult},
};
use alloy_primitives::{Address, U256};
use std::{
    collections::{BTreeMap, btree_map::Entry},
    fmt,
    sync::Arc,
};

use super::super::{CallHookError, RegisterHookError};
use super::{CallHandler, CallOutput, ExitStatus};

/// The EVM call operation that reached the hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallScheme {
    Call,
    /// REVM's precompile input intentionally combines these two indirect call
    /// forms because their execution context is identical at that boundary.
    CallCodeOrDelegateCall,
    StaticCall,
}

/// A version-independent view of an EVM call frame.
#[derive(Clone, Copy, Debug)]
pub struct CallInput<'a> {
    pub caller: Address,
    /// Address whose storage/value context is active for the call.
    pub target: Address,
    /// Address containing the invoked code. The router dispatches on this
    /// field, which matters for DELEGATECALL and CALLCODE.
    pub code_address: Address,
    pub value: U256,
    pub input: &'a [u8],
    pub gas_limit: u64,
    pub scheme: CallScheme,
}

/// Routes reserved-address calls to Arcology runtime handlers.
#[derive(Clone, Default)]
pub struct CallRouter {
    // Keep registration and collision checks ordered without building an address list.
    pub(in crate::execution::evm) handlers: BTreeMap<Address, Arc<dyn CallHandler>>,
}

impl fmt::Debug for CallRouter {
    /// Lists registered targets without requiring handlers to implement `Debug`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallRouter")
            .field("addresses", &self.handlers.keys())
            .finish()
    }
}

impl CallRouter {
    /// Creates an empty router.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a handler at an EVM address.
    /// Duplicate addresses return an error and keep the existing handler.
    pub fn register<H>(&mut self, address: Address, handler: H) -> Result<(), RegisterHookError>
    where
        H: CallHandler + 'static,
    {
        match self.handlers.entry(address) {
            Entry::Occupied(_) => Err(RegisterHookError { address }),
            Entry::Vacant(entry) => {
                entry.insert(Arc::new(handler));
                Ok(())
            }
        }
    }

    /// Routes by code address and checks the handler's reported gas usage.
    /// Returns `None` for unregistered targets, or `Some(output)` for handled calls.
    pub fn dispatch(
        &self,
        input: &CallInput<'_>,
        internals: &mut EvmInternals<'_>,
    ) -> Result<Option<CallOutput>, CallHookError> {
        let Some(handler) = self.handlers.get(&input.code_address) else {
            return Ok(None);
        };

        let output = handler.call(input, internals)?;
        if output.gas_used > input.gas_limit {
            return Err(CallHookError::OutOfGas {
                gas_limit: input.gas_limit,
                gas_used: output.gas_used,
            });
        }

        Ok(Some(output))
    }
}

/// Dispatches a REVM call frame through the Arcology hook router.
///
/// This function is installed as a stateful, non-cacheable precompile by
/// [`ArcologyEvmFactory`](super::super::ArcologyEvmFactory). The `PrecompileInput`
/// carries the journal-backed `EvmInternals` passed to stateful call hooks.
pub fn dispatch_precompile(
    router: &CallRouter,
    mut input: PrecompileInput<'_>,
) -> PrecompileResult {
    let scheme = if input.is_static_call() {
        CallScheme::StaticCall
    } else if input.is_direct_call() {
        CallScheme::Call
    } else {
        CallScheme::CallCodeOrDelegateCall
    };

    let hook_input = CallInput {
        caller: input.caller,
        target: input.target_address,
        code_address: input.bytecode_address,
        value: input.value,
        input: input.data,
        gas_limit: input.gas,
        scheme,
    };

    match router.dispatch(&hook_input, &mut input.internals) {
        Ok(Some(output)) => match output.status {
            ExitStatus::Return => Ok(PrecompileOutput::new(
                output.gas_used,
                output.output,
                input.reservoir,
            )),
            ExitStatus::Revert => Ok(PrecompileOutput::revert(
                output.gas_used,
                output.output,
                input.reservoir,
            )),
        },
        Ok(None) => Err(PrecompileError::Fatal(format!(
            "Arcology precompile at {:?} has no registered call hook",
            input.bytecode_address
        ))),
        Err(CallHookError::OutOfGas { .. }) => Ok(PrecompileOutput::halt(
            PrecompileHalt::OutOfGas,
            input.reservoir,
        )),
        Err(CallHookError::Handler(message)) => Err(PrecompileError::Fatal(message)),
    }
}
