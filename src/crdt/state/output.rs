pub struct TransactionOutput<K, V> {
    pub tx_id: u64,
    pub gas_used: u64,
    pub records: Vec<(K, V)>,
}
