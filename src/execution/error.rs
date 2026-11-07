//! Errors from EVM call handling, hook registration, and account decoding.

use std::{error::Error, fmt};

use alloy_evm::revm::{database_interface::DBErrorMarker, precompile::PrecompileId};
use alloy_primitives::Address;

/// Failures returned by a call handler or detected by the router.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallHookError {
    /// The handler reported gas usage above the call's supplied limit.
    OutOfGas { gas_limit: u64, gas_used: u64 },
    /// Fatal handler failure, not a contract revert; use `CallOutput::reverted` for reverts.
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

/// Duplicate registration rejected without replacing the existing handler.
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

/// An Arcology handler targets an address occupied by a backend precompile.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrecompileCollisionError {
    /// Requested Arcology call target.
    pub address: Address,
    /// Backend precompile already installed at that address.
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

/// Encoded `AccountInfo` has an invalid length or code-presence flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AccountDecodeError;

impl fmt::Display for AccountDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid encoded AccountInfo")
    }
}

impl Error for AccountDecodeError {}
// Allow account decoding failures to be used as REVM database errors.
impl DBErrorMarker for AccountDecodeError {}
