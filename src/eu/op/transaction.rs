//! Arcology-owned transaction environment for the OP-backed EVM.

use alloy_evm::{
    FromRecoveredTx, FromTxWithEncoded, IntoTxEnv,
    revm::context::{Transaction, TxEnv},
};
use alloy_op_evm::{OpTx, block::OpTxEnv};
use alloy_primitives::{Address, B256, Bytes, TxKind, U256};
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

impl Transaction for ArcologyTx {
    type AccessListItem<'a>
        = <OpTx as Transaction>::AccessListItem<'a>
    where
        Self: 'a;
    type Authorization<'a>
        = <OpTx as Transaction>::Authorization<'a>
    where
        Self: 'a;

    fn tx_type(&self) -> u8 {
        self.op_tx.tx_type()
    }

    fn caller(&self) -> Address {
        self.op_tx.caller()
    }

    fn gas_limit(&self) -> u64 {
        self.op_tx.gas_limit()
    }

    fn value(&self) -> U256 {
        self.op_tx.value()
    }

    fn input(&self) -> &Bytes {
        self.op_tx.input()
    }

    fn nonce(&self) -> u64 {
        Transaction::nonce(&self.op_tx)
    }

    fn kind(&self) -> TxKind {
        self.op_tx.kind()
    }

    fn chain_id(&self) -> Option<u64> {
        self.op_tx.chain_id()
    }

    fn gas_price(&self) -> u128 {
        self.op_tx.gas_price()
    }

    fn access_list(&self) -> Option<impl Iterator<Item = Self::AccessListItem<'_>>> {
        self.op_tx.access_list()
    }

    fn blob_versioned_hashes(&self) -> &[B256] {
        self.op_tx.blob_versioned_hashes()
    }

    fn max_fee_per_blob_gas(&self) -> u128 {
        self.op_tx.max_fee_per_blob_gas()
    }

    fn authorization_list_len(&self) -> usize {
        self.op_tx.authorization_list_len()
    }

    fn authorization_list(&self) -> impl Iterator<Item = Self::Authorization<'_>> {
        self.op_tx.authorization_list()
    }

    fn max_priority_fee_per_gas(&self) -> Option<u128> {
        self.op_tx.max_priority_fee_per_gas()
    }

    fn effective_gas_price(&self, base_fee: u128) -> u128 {
        self.op_tx.effective_gas_price(base_fee)
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
