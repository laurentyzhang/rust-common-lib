use alloy_evm::revm::state::{EvmStorage, EvmStorageSlot};
use alloy_primitives::{U256, map::hash_map::Entry};
use rayon::prelude::*;

use super::StateDb;
use crate::eu::{ApiAddress, Field, KeyCodec};
use crate::namespace::Namespace;

impl<DB> StateDb<DB> {}
