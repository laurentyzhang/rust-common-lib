use alloy_evm::revm::{
    DatabaseCommit,
    state::{Account, EvmState, EvmStorageSlot, TransactionId},
};
use alloy_primitives::{Address, U256};
use rayon::prelude::*;
use rust_common_lib::execution::evm::{StateDb, storage::AccountStorage};
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

const SAMPLES: usize = 11;
const SAMPLE_TIME: Duration = Duration::from_millis(20);
type Merge = fn(EvmState, EvmState) -> EvmState;

struct Workload {
    name: &'static str,
    accounts: usize,
    updated_accounts: usize,
    new_accounts: usize,
    slots: usize,
    updated_slots: usize,
    new_slots: usize,
    reset_accounts: bool,
}

fn account(index: usize, slots: usize, incoming: bool) -> Account {
    let mut account = Account::default();
    account.info.balance = U256::from(index + 1);
    account.set_current_info_as_original();
    account.info.nonce = u64::from(incoming);
    account.mark_touch();
    for key in 0..slots {
        account.storage.insert(
            U256::from(key),
            EvmStorageSlot::new(U256::from(key + 1), TransactionId::ZERO),
        );
    }
    account
}

fn input(workload: &Workload) -> (EvmState, EvmState) {
    let mut base = EvmState::default();
    let mut incoming = EvmState::default();
    for index in 0..workload.accounts {
        let address = Address::from_word(U256::from(index + 1).into());
        base.insert(address, account(index, workload.slots, false));
        if index < workload.updated_accounts {
            let mut update = account(index, 0, true);
            for key in 0..workload.updated_slots {
                update.storage.insert(
                    U256::from(key),
                    EvmStorageSlot::new_changed(
                        U256::from(key + 1),
                        U256::from(key + 100),
                        TransactionId::ZERO,
                    ),
                );
            }
            for key in workload.slots..workload.slots + workload.new_slots {
                update.storage.insert(
                    U256::from(key),
                    EvmStorageSlot::new_changed(
                        U256::ZERO,
                        U256::from(key + 100),
                        TransactionId::ZERO,
                    ),
                );
            }
            if workload.reset_accounts && index % 4 == 0 {
                update.mark_created();
            }
            if workload.reset_accounts && index % 4 == 1 {
                update.mark_selfdestruct();
            }
            incoming.insert(address, update);
        }
    }
    for index in workload.accounts..workload.accounts + workload.new_accounts {
        let address = Address::from_word(U256::from(index + 1).into());
        incoming.insert(
            address,
            account(index, workload.updated_slots + workload.new_slots, true),
        );
    }
    (base, incoming)
}

// Measures the current StateDb commit implementation.
fn merge_vector(base: EvmState, incoming: EvmState) -> EvmState {
    let mut state = StateDb::new(base);
    state.commit(incoming);
    assert!(state.commit_error().is_none(), "{:?}", state.commit_error());
    state.into_evm_state()
}

// Measures a retain-based parallel merge that keeps the overlay's storage slots.
fn merge_bridge(mut base: EvmState, mut incoming: EvmState) -> EvmState {
    incoming.retain(|address, update| {
        let Some(account) = base.get_mut(address) else {
            base.insert(*address, std::mem::take(update));
            return false;
        };
        update.storage.retain(|key, slot| {
            if account.storage.contains_key(key) {
                return true;
            }
            account.storage.insert(*key, std::mem::take(slot));
            false
        });
        true
    });

    base.iter_mut()
        .filter_map(|(address, account)| incoming.remove(address).map(|update| (account, update)))
        .par_bridge()
        .for_each(|(account, mut update)| {
            if account.is_created() {
                update.mark_created();
            }
            for (key, value) in update.storage.drain() {
                *account
                    .storage
                    .get_mut(&key)
                    .expect("slot exists after preparation") = value;
            }
            std::mem::swap(&mut account.storage, &mut update.storage);
            *account = update;
        });
    base
}

// Computes the expected overlay state by merging storage serially.
fn merge_serial(mut base: EvmState, incoming: EvmState) -> EvmState {
    for (address, mut update) in incoming {
        if let Some(mut previous) = base.remove(&address) {
            if previous.is_created() {
                update.mark_created();
            }
            previous.storage.extend(std::mem::take(&mut update.storage));
            update.storage = previous.storage;
        }
        base.insert(address, update);
    }
    base
}

// Input cloning and result destruction are outside the measured interval.
fn measure(merge: Merge, base: &EvmState, incoming: &EvmState, iterations: usize) -> f64 {
    let mut elapsed = Duration::ZERO;
    for _ in 0..iterations {
        let base = base.clone();
        let incoming = incoming.clone();
        let start = Instant::now();
        let result = merge(black_box(base), black_box(incoming));
        elapsed += start.elapsed();
        black_box(&result);
        drop(result);
    }
    elapsed.as_secs_f64() / iterations as f64
}

fn median(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[samples.len() / 2]
}

fn main() {
    let workloads = [
        Workload {
            name: "tiny",
            accounts: 8,
            updated_accounts: 4,
            new_accounts: 0,
            slots: 8,
            updated_slots: 4,
            new_slots: 0,
            reset_accounts: false,
        },
        Workload {
            name: "many_accounts",
            accounts: 4096,
            updated_accounts: 4096,
            new_accounts: 0,
            slots: 16,
            updated_slots: 8,
            new_slots: 0,
            reset_accounts: false,
        },
        Workload {
            name: "large_storage",
            accounts: 64,
            updated_accounts: 64,
            new_accounts: 0,
            slots: 4096,
            updated_slots: 2048,
            new_slots: 0,
            reset_accounts: false,
        },
        Workload {
            name: "one_account",
            accounts: 1,
            updated_accounts: 1,
            new_accounts: 0,
            slots: 65536,
            updated_slots: 32768,
            new_slots: 0,
            reset_accounts: false,
        },
        Workload {
            name: "sparse_updates",
            accounts: 32768,
            updated_accounts: 64,
            new_accounts: 0,
            slots: 16,
            updated_slots: 4,
            new_slots: 0,
            reset_accounts: false,
        },
        Workload {
            name: "mixed_new_keys",
            accounts: 2048,
            updated_accounts: 1024,
            new_accounts: 512,
            slots: 32,
            updated_slots: 16,
            new_slots: 8,
            reset_accounts: false,
        },
        Workload {
            name: "new_accounts_only",
            accounts: 1024,
            updated_accounts: 0,
            new_accounts: 1024,
            slots: 32,
            updated_slots: 16,
            new_slots: 0,
            reset_accounts: false,
        },
        Workload {
            name: "creation_deletion",
            accounts: 1024,
            updated_accounts: 1024,
            new_accounts: 0,
            slots: 32,
            updated_slots: 16,
            new_slots: 8,
            reset_accounts: true,
        },
    ];
    let available = std::thread::available_parallelism().unwrap().get();
    let mut threads = vec![1, 4.min(available), available];
    threads.sort_unstable();
    threads.dedup();
    let pools: Vec<_> = threads
        .iter()
        .map(|&count| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(count)
                .build()
                .unwrap()
        })
        .collect();

    eprintln!(
        "Release benchmark: {SAMPLES} alternating samples; setup/drop excluded; logical CPUs={available}"
    );
    println!(
        "scenario,threads,base_accounts,incoming_accounts,base_slots,incoming_slots,iterations,vector_us,bridge_us,bridge_over_vector"
    );
    for workload in workloads {
        let (base, incoming) = input(&workload);
        let expected = merge_serial(base.clone(), incoming.clone());
        let base_slots: usize = base.values().map(|account| account.storage.len()).sum();
        let incoming_slots: usize = incoming.values().map(|account| account.storage.len()).sum();
        for (count, pool) in threads.iter().zip(&pools) {
            pool.install(|| {
                assert_eq!(
                    merge_vector(base.clone(), incoming.clone()),
                    expected,
                    "vector: {}",
                    workload.name
                );
                assert_eq!(
                    merge_bridge(base.clone(), incoming.clone()),
                    expected,
                    "bridge: {}",
                    workload.name
                );
                let mut warm_vector = Vec::new();
                let mut warm_bridge = Vec::new();
                for _ in 0..3 {
                    warm_vector.push(measure(merge_vector, &base, &incoming, 1));
                    warm_bridge.push(measure(merge_bridge, &base, &incoming, 1));
                }
                let estimate = median(&mut warm_vector).min(median(&mut warm_bridge));
                let iterations = (SAMPLE_TIME.as_secs_f64() / estimate)
                    .ceil()
                    .clamp(1.0, 1024.0) as usize;
                let mut vector = Vec::new();
                let mut bridge = Vec::new();
                for sample in 0..SAMPLES {
                    if sample % 2 == 0 {
                        vector.push(measure(merge_vector, &base, &incoming, iterations));
                        bridge.push(measure(merge_bridge, &base, &incoming, iterations));
                    } else {
                        bridge.push(measure(merge_bridge, &base, &incoming, iterations));
                        vector.push(measure(merge_vector, &base, &incoming, iterations));
                    }
                }
                let vector = median(&mut vector);
                let bridge = median(&mut bridge);
                println!(
                    "{},{count},{},{},{base_slots},{incoming_slots},{iterations},{:.3},{:.3},{:.3}",
                    workload.name,
                    base.len(),
                    incoming.len(),
                    vector * 1e6,
                    bridge * 1e6,
                    bridge / vector
                );
            });
        }
    }
}
