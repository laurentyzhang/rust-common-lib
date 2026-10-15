//! Arcology-owned transaction environment for the OP-backed EVM.

use alloy_evm::{FromRecoveredTx, FromTxWithEncoded, IntoTxEnv, revm::context::TxEnv};
use alloy_op_evm::{OpTx, block::OpTxEnv};
use alloy_primitives::{Address, Bytes};
use op_alloy::consensus::OpTxEnvelope;
use op_revm::OpTransaction;

/// Transaction environment accepted by [`super::ArcologyEvm`].
///
/// The OP transaction representation remains private so callers only construct and pass
/// Arcology types.
#[derive(Clone, Debug, Default)]
pub struct ArcologyTx {
    op_tx: OpTx,
}

impl From<TxEnv> for ArcologyTx {
    fn from(tx_env: TxEnv) -> Self {
        let mut op_tx = OpTx::default();
        op_tx.base = tx_env;
        Self { op_tx }
    }
}

impl IntoTxEnv<Self> for ArcologyTx {
    fn into_tx_env(self) -> Self {
        self
    }
}

#[doc(hidden)]
impl FromRecoveredTx<OpTxEnvelope> for ArcologyTx {
    fn from_recovered_tx(tx: &OpTxEnvelope, sender: Address) -> Self {
        Self {
            op_tx: OpTx::from_recovered_tx(tx, sender),
        }
    }
}

#[doc(hidden)]
impl FromTxWithEncoded<OpTxEnvelope> for ArcologyTx {
    fn from_encoded_tx(tx: &OpTxEnvelope, sender: Address, encoded: Bytes) -> Self {
        Self {
            op_tx: OpTx::from_encoded_tx(tx, sender, encoded),
        }
    }
}

impl OpTxEnv for ArcologyTx {
    fn encoded_bytes(&self) -> Option<&Bytes> {
        self.op_tx.encoded_bytes()
    }
}

#[doc(hidden)]
impl From<ArcologyTx> for OpTransaction<TxEnv> {
    fn from(tx: ArcologyTx) -> Self {
        tx.op_tx.into()
    }
}
