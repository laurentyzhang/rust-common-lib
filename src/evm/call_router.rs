use alloy_primitives::{Address, Bytes, U256};
use std::{collections::HashMap, error::Error, fmt, sync::Arc};

/// Arcology's reserved EVM call targets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum ArcologyApi {
    Io = 0x60,
    Container = 0x84,
    CumulativeU256 = 0x85,
    CumulativeI256 = 0x86,
    Runtime = 0xa0,
    Multiprocess = 0xb0,
}

impl ArcologyApi {
    pub const ALL: [Self; 6] = [
        Self::Io,
        Self::Container,
        Self::CumulativeU256,
        Self::CumulativeI256,
        Self::Runtime,
        Self::Multiprocess,
    ];

    pub fn address(self) -> Address {
        arcology_address(self as u8)
    }

    pub fn from_address(address: Address) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|api| api.address() == address)
    }
}

/// Builds the canonical low Arcology address for a one-byte suffix.
pub fn arcology_address(suffix: u8) -> Address {
    let mut bytes = [0_u8; 20];
    bytes[19] = suffix;
    Address::from(bytes)
}

/// The EVM call operation that reached the hook.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CallScheme {
    #[default]
    Call,
    CallCode,
    DelegateCall,
    /// REVM's precompile input intentionally combines these two indirect call
    /// forms because their execution context is identical at that boundary.
    CallCodeOrDelegateCall,
    StaticCall,
}

/// A version-independent view of an EVM call frame.
#[derive(Clone, Copy, Debug)]
pub struct CallHookInput<'a> {
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

impl CallHookInput<'_> {
    pub fn is_static(&self) -> bool {
        self.scheme == CallScheme::StaticCall
    }
}

/// How the intercepted call completed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    Return,
    Revert,
}

/// Result produced by an Arcology call handler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallHookOutput {
    pub status: ExitStatus,
    pub gas_used: u64,
    pub output: Bytes,
}

impl CallHookOutput {
    pub fn returned(gas_used: u64, output: impl Into<Bytes>) -> Self {
        Self {
            status: ExitStatus::Return,
            gas_used,
            output: output.into(),
        }
    }

    pub fn reverted(gas_used: u64, output: impl Into<Bytes>) -> Self {
        Self {
            status: ExitStatus::Revert,
            gas_used,
            output: output.into(),
        }
    }
}

/// Router result. Non-Arcology calls continue through the normal EVM path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallHookOutcome {
    Continue,
    Handled(CallHookOutput),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallHookError {
    OutOfGas { gas_limit: u64, gas_used: u64 },
    Handler(String),
}

impl fmt::Display for CallHookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfGas {
                gas_limit,
                gas_used,
            } => write!(
                f,
                "Arcology call used {gas_used} gas but only {gas_limit} was supplied"
            ),
            Self::Handler(message) => f.write_str(message),
        }
    }
}

impl Error for CallHookError {}

/// Handles calls for one reserved address.
pub trait CallHook: Send + Sync {
    fn call(&self, input: &CallHookInput<'_>) -> Result<CallHookOutput, CallHookError>;
}

/// Adapts a closure into a [`CallHook`].
pub struct ClosureCallHook<F>(F);

impl<F> ClosureCallHook<F> {
    pub fn new(handler: F) -> Self {
        Self(handler)
    }
}

impl<F> CallHook for ClosureCallHook<F>
where
    F: Fn(&CallHookInput<'_>) -> Result<CallHookOutput, CallHookError> + Send + Sync,
{
    fn call(&self, input: &CallHookInput<'_>) -> Result<CallHookOutput, CallHookError> {
        (self.0)(input)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegisterHookError {
    pub address: Address,
}

impl fmt::Display for RegisterHookError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a call hook is already registered at {:?}", self.address)
    }
}

impl Error for RegisterHookError {}

/// Routes reserved-address calls to Arcology runtime handlers.
#[derive(Clone, Default)]
pub struct CallHookRouter {
    handlers: HashMap<Address, Arc<dyn CallHook>>,
}

impl fmt::Debug for CallHookRouter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallHookRouter")
            .field("registered_addresses", &self.registered_addresses())
            .finish()
    }
}

impl CallHookRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<H>(&mut self, api: ArcologyApi, handler: H) -> Result<(), RegisterHookError>
    where
        H: CallHook + 'static,
    {
        self.register_address(api.address(), handler)
    }

    pub fn register_address<H>(
        &mut self,
        address: Address,
        handler: H,
    ) -> Result<(), RegisterHookError>
    where
        H: CallHook + 'static,
    {
        if self.handlers.contains_key(&address) {
            return Err(RegisterHookError { address });
        }

        self.handlers.insert(address, Arc::new(handler));
        Ok(())
    }

    pub fn dispatch(&self, input: &CallHookInput<'_>) -> Result<CallHookOutcome, CallHookError> {
        let Some(handler) = self.handlers.get(&input.code_address) else {
            return Ok(CallHookOutcome::Continue);
        };

        let output = handler.call(input)?;
        if output.gas_used > input.gas_limit {
            return Err(CallHookError::OutOfGas {
                gas_limit: input.gas_limit,
                gas_used: output.gas_used,
            });
        }

        Ok(CallHookOutcome::Handled(output))
    }

    pub fn contains(&self, address: Address) -> bool {
        self.handlers.contains_key(&address)
    }

    pub fn registered_addresses(&self) -> Vec<Address> {
        let mut addresses: Vec<_> = self.handlers.keys().copied().collect();
        addresses.sort_unstable();
        addresses
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(target: Address, gas_limit: u64) -> CallHookInput<'static> {
        CallHookInput {
            caller: arcology_address(0x01),
            target,
            code_address: target,
            value: U256::ZERO,
            input: &[0xde, 0xad, 0xbe, 0xef],
            gas_limit,
            scheme: CallScheme::Call,
        }
    }

    #[test]
    fn recognizes_arcology_addresses() {
        for api in ArcologyApi::ALL {
            assert_eq!(ArcologyApi::from_address(api.address()), Some(api));
        }

        assert_eq!(ArcologyApi::from_address(arcology_address(0x42)), None);
    }

    #[test]
    fn normal_calls_continue() {
        let router = CallHookRouter::new();
        let outcome = router
            .dispatch(&input(arcology_address(0x42), 100))
            .unwrap();

        assert_eq!(outcome, CallHookOutcome::Continue);
    }

    #[test]
    fn registered_runtime_call_is_handled() {
        let mut router = CallHookRouter::new();
        router
            .register(
                ArcologyApi::Runtime,
                ClosureCallHook::new(|call: &CallHookInput<'_>| {
                    assert_eq!(call.input, &[0xde, 0xad, 0xbe, 0xef]);
                    Ok(CallHookOutput::returned(21, Bytes::from(vec![7_u8])))
                }),
            )
            .unwrap();

        let outcome = router
            .dispatch(&input(ArcologyApi::Runtime.address(), 100))
            .unwrap();

        assert_eq!(
            outcome,
            CallHookOutcome::Handled(CallHookOutput::returned(21, Bytes::from(vec![7_u8])))
        );
    }

    #[test]
    fn revert_is_a_handled_result() {
        let mut router = CallHookRouter::new();
        router
            .register(
                ArcologyApi::Runtime,
                ClosureCallHook::new(|_: &CallHookInput<'_>| {
                    Ok(CallHookOutput::reverted(
                        30,
                        Bytes::from_static(b"runtime revert"),
                    ))
                }),
            )
            .unwrap();

        let outcome = router
            .dispatch(&input(ArcologyApi::Runtime.address(), 100))
            .unwrap();

        assert_eq!(
            outcome,
            CallHookOutcome::Handled(CallHookOutput::reverted(
                30,
                Bytes::from_static(b"runtime revert")
            ))
        );
    }

    #[test]
    fn handler_cannot_use_more_than_supplied_gas() {
        let mut router = CallHookRouter::new();
        router
            .register(
                ArcologyApi::Runtime,
                ClosureCallHook::new(|_: &CallHookInput<'_>| {
                    Ok(CallHookOutput::returned(101, Bytes::new()))
                }),
            )
            .unwrap();

        assert_eq!(
            router.dispatch(&input(ArcologyApi::Runtime.address(), 100)),
            Err(CallHookError::OutOfGas {
                gas_limit: 100,
                gas_used: 101,
            })
        );
    }

    #[test]
    fn duplicate_registration_is_rejected() {
        let mut router = CallHookRouter::new();
        router
            .register(
                ArcologyApi::Runtime,
                ClosureCallHook::new(|_: &CallHookInput<'_>| {
                    Ok(CallHookOutput::returned(0, Bytes::new()))
                }),
            )
            .unwrap();

        let result = router.register(
            ArcologyApi::Runtime,
            ClosureCallHook::new(|_: &CallHookInput<'_>| {
                Ok(CallHookOutput::returned(0, Bytes::new()))
            }),
        );

        assert_eq!(
            result,
            Err(RegisterHookError {
                address: ArcologyApi::Runtime.address()
            })
        );
    }
}
