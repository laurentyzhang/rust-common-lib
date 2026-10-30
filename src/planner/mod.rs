pub mod history;
pub mod resolver;
pub mod scheduler;

use std::collections::{BTreeMap, BTreeSet};

use alloy_evm::{
    block::BlockExecutionError,
    revm::{
        Database,
        state::{Account, EvmState, EvmStorageSlot},
    },
};
use alloy_primitives::{Address, U256};

use history::Field;
use resolver::{ConflictChecker, ConflictResult};
use scheduler::{Generation, GreedyScheduler, Job, Scheduler, SchedulerConfig};

/// Scheduling and conflict detection state for one generation.
pub struct Planner<DB: Database, K> {
    pub db: DB,
    pub scheduler: GreedyScheduler,
    pub checker: ConflictChecker<K>,
    contracts: BTreeMap<u64, Option<Address>>, // Tx id to contract mapping
}

// Planner manages the scheduling and conflict detection for transactions within a generation.
impl<DB: Database, K> Planner<DB, K> {
    pub fn new(db: DB, config: SchedulerConfig) -> Self {
        Self {
            db,
            scheduler: GreedyScheduler::with_config(config),
            checker: ConflictChecker::new(),
            contracts: BTreeMap::new(),
        }
    }

    /// Feeds contract history into callee profiles before building the plan.
    pub fn plan<Tx>(
        &mut self,
        jobs: impl IntoIterator<Item = Job<Tx>>,
    ) -> Result<Vec<Generation<Tx>>, BlockExecutionError> {
        let jobs = jobs.into_iter().collect::<Vec<_>>();
        let mut callees = BTreeMap::<Address, BTreeSet<u64>>::new();
        for job in &jobs {
            if let (Some(contract), Some(callee)) = (job.contract, job.callee) {
                callees.entry(contract).or_default().insert(callee);
            }
        }

        // For each contract, determine its conflict peers based on the history of callees.
        for (&contract, ids) in &callees {
            let peers = self.load_history(contract)?;
            let peers = peers
                .iter()
                .filter_map(|peer| callees.get(peer))
                .flatten()
                .copied()
                .collect::<BTreeSet<_>>();
            for &callee in ids {
                self.scheduler.add_conflict_peers(callee, peers.clone());
            }
        }

        // Record the mapping from transaction IDs to their associated contracts.
        self.contracts = jobs
            .iter()
            .map(|job| (job.id.global, job.contract))
            .collect();

        // Build the schedule based on the collected conflict information.
        Ok(self.scheduler.schedule(jobs))
    }

    /// Loads the append-only list of distinct peers stored under this contract.
    fn load_history(&mut self, contract: Address) -> Result<Vec<Address>, BlockExecutionError> {
        let count = self
            .db
            .storage(contract, Field::Count.slot(0))
            .map_err(BlockExecutionError::other)?;
        let count: u64 = count
            .try_into()
            .map_err(|_| BlockExecutionError::msg("conflict history count exceeds u64"))?;
        let mut peers = Vec::new();
        for index in 0..count {
            let value = self
                .db
                .storage(contract, Field::Entry.slot(index))
                .map_err(BlockExecutionError::other)?;
            let bytes = value.to_be_bytes::<32>();
            if bytes[..12] != [0; 12] {
                return Err(BlockExecutionError::msg("invalid conflict history address"));
            }
            peers.push(Address::from_slice(&bytes[12..]));
        }
        Ok(peers)
    }
}

impl<DB, K> Planner<DB, K>
where
    DB: Database,
{
    /// Converts a generation's detection result into history updates for the block committer.
    pub fn resolve(
        &mut self,
        mut result: ConflictResult<K>,
    ) -> Result<ConflictResult<K>, BlockExecutionError> {
        let mut peers = BTreeMap::<Address, BTreeSet<Address>>::new();
        for &(left, right) in &result.conflicts {
            let left = self.contracts.get(&left).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;
            let right = self.contracts.get(&right).ok_or_else(|| {
                BlockExecutionError::msg("conflicting transaction was not scheduled")
            })?;
            if let (Some(left), Some(right)) = (left, right) {
                peers.entry(*left).or_default().insert(*right);
                peers.entry(*right).or_default().insert(*left);
            }
        }

        let mut updates = EvmState::default();
        for (contract, detected) in peers {
            let history = self.load_history(contract)?;
            let new_peers = detected
                .into_iter()
                .filter(|peer| !history.contains(peer))
                .collect::<Vec<_>>();
            if new_peers.is_empty() {
                continue;
            }
            let info = self
                .db
                .basic(contract)
                .map_err(BlockExecutionError::other)?
                .unwrap_or_default();
            let mut account = Account::from(info).with_touched_mark();
            for (offset, peer) in new_peers.iter().enumerate() {
                let slot = Field::Entry.slot((history.len() + offset) as u64);
                let original = self
                    .db
                    .storage(contract, slot)
                    .map_err(BlockExecutionError::other)?;
                account.storage.insert(
                    slot,
                    EvmStorageSlot::new_changed(
                        original,
                        U256::from_be_slice(peer.as_slice()),
                        Default::default(),
                    ),
                );
            }
            account.storage.insert(
                Field::Count.slot(0),
                EvmStorageSlot::new_changed(
                    U256::from(history.len()),
                    U256::from(history.len() + new_peers.len()),
                    Default::default(),
                ),
            );
            updates.insert(contract, account);
        }
        result.history = updates;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
