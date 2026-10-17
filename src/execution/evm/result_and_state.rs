use alloy_evm::revm::context_interface::result::{HaltReason, ResultAndState};
use rayon::prelude::*;

const MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER: usize = 1_024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeableResultAndState<H = HaltReason> {
    pub result_and_state: ResultAndState<H>,
}

impl<H> MergeableResultAndState<H> {
    pub fn new(result_and_state: ResultAndState<H>) -> Self {
        Self { result_and_state }
    }
}

impl<H: Send> MergeableResultAndState<H> {
    pub fn merge(&mut self, other: Self) {
        let mut other_state = other.result_and_state.state;

        if self.result_and_state.state.len() < other_state.len() {
            std::mem::swap(&mut self.result_and_state.state, &mut other_state);
        }

        self.result_and_state.state.extend(other_state);
    }

    pub fn merge_all(&mut self, mut others: Vec<Self>) {
        while Self::should_merge_parallel(&others) {
            others = others
                .into_par_iter()
                .chunks(2)
                .map(|mut pair| {
                    let mut left = pair.remove(0);
                    if let Some(right) = pair.pop() {
                        left.merge(right);
                    }
                    left
                })
                .collect();
        }

        for other in others {
            self.merge(other);
        }
    }

    fn should_merge_parallel(results: &[Self]) -> bool {
        let pairs = results.len() / 2;
        let workers = rayon::current_num_threads().min(pairs);

        if workers < 2 {
            return false;
        }

        let lesser_entries: usize = results
            .chunks_exact(2)
            .map(|pair| {
                pair[0]
                    .result_and_state
                    .state
                    .len()
                    .min(pair[1].result_and_state.state.len())
            })
            .sum();

        lesser_entries >= MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER * workers
    }
}

#[cfg(test)]
mod tests {
    use alloy_evm::revm::{
        context_interface::result::{ExecutionResult, Output, ResultGas, SuccessReason},
        primitives::{Address, Bytes},
        state::{Account, AccountInfo, EvmState},
    };

    use super::*;

    fn result(output: u8, state: EvmState) -> MergeableResultAndState {
        MergeableResultAndState::new(ResultAndState::new(
            ExecutionResult::Success {
                reason: SuccessReason::Stop,
                gas: ResultGas::default(),
                logs: Vec::new(),
                output: Output::Call(Bytes::from(vec![output])),
            },
            state,
        ))
    }

    fn address(index: u64) -> Address {
        let mut address = [0; 20];
        address[12..].copy_from_slice(&index.to_be_bytes());
        Address::from(address)
    }

    #[test]
    fn merge_keeps_parent_result_and_combines_state() {
        let mut left_state = EvmState::default();
        left_state.insert(address(1), Account::from(AccountInfo::default()));

        let mut right_state = EvmState::default();
        right_state.insert(address(2), Account::from(AccountInfo::default()));

        let mut merged = result(1, left_state);
        merged.merge(result(2, right_state));

        assert_eq!(
            merged.result_and_state.result.output(),
            Some(&Bytes::from(vec![1]))
        );
        assert_eq!(merged.result_and_state.state.len(), 2);
    }

    #[test]
    fn merge_all_uses_parallel_tree_for_large_states() {
        let others: Vec<_> = (0..4)
            .map(|group| {
                let first = group as u64 * MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER as u64;
                let mut state = EvmState::default();

                for offset in 0..MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER as u64 {
                    state.insert(
                        address(first + offset + 1),
                        Account::from(AccountInfo::default()),
                    );
                }

                result(group, state)
            })
            .collect();

        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(2)
            .build()
            .unwrap();
        let mut merged = result(0, EvmState::default());
        pool.install(|| {
            assert!(MergeableResultAndState::should_merge_parallel(&others));
            merged.merge_all(others);
        });

        assert_eq!(
            merged.result_and_state.result.output(),
            Some(&Bytes::from(vec![0]))
        );
        assert_eq!(
            merged.result_and_state.state.len(),
            4 * MIN_PARALLEL_MERGE_ENTRIES_PER_WORKER
        );
    }
}
