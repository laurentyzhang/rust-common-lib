use alloy_evm::revm::state::{EvmStorage, EvmStorageSlot};
use alloy_primitives::{U256, map::hash_map::Entry};
use rayon::prelude::*;

use super::StateDb;
use crate::execution::{Property, ScalarKeyCodec};
use crate::namespace::Namespace;

impl<DB> StateDb<DB> {}
