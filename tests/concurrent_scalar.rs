//! Run manually with `cargo test --test concurrent_scalar -- --ignored --nocapture`.
//! Set `ARCOLOGY_CONCURRENT_ROOT` and `SOLC` to override the checkout and compiler paths.

use alloy_evm::{
    Evm, EvmEnv, EvmFactory,
    revm::{DatabaseCommit, context::TxEnv, state::EvmState},
};
use alloy_primitives::{Address, Bytes, TxKind, U256, keccak256};
use rust_common_lib::execution::{
    ApiAddress, ArcologyEvmFactory, ArcologyTx, CallRouter, StateDb, U256Handler,
};
use std::{path::PathBuf, process::Command, str::FromStr};

#[test]
#[ignore = "requires the concurrent checkout and Solidity compiler"]
fn concurrent_scalar_contracts() {
    let root = std::env::var_os("ARCOLOGY_CONCURRENT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join("..")
                .join("go")
                .join("concurrent")
        });
    let solc = std::env::var_os("SOLC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            if cfg!(windows) {
                root.join("test")
                    .join("solc")
                    .join("windows-amd64")
                    .join("0.8.38")
                    .join("solc.exe")
            } else {
                PathBuf::from("solc")
            }
        });

    let output = Command::new(&solc)
        .current_dir(&root)
        .args(["--base-path", ".", "--bin"])
        .arg(
            PathBuf::from("test")
                .join("unit")
                .join("crdt")
                .join("scalar")
                .join("u256Cum.t.sol"),
        )
        .arg(
            PathBuf::from("test")
                .join("unit")
                .join("crdt")
                .join("scalar")
                .join("simple.sol"),
        )
        .output()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", solc.display()));
    assert!(
        output.status.success(),
        "solc failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let compiled = String::from_utf8(output.stdout).expect("solc output is not UTF-8");

    let mut router = CallRouter::new();
    router
        .register(ApiAddress::U256.address(), U256Handler)
        .unwrap();
    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let caller = Address::with_last_byte(0x11);

    for (contract, method, expected) in [
        ("CumulativeU256InMappingTest", "testInitialState()", None),
        ("CumulativeU256Test", "testCall()", None),
        ("MultiCommutativeTest", "testCheck()", None),
        ("ViewQuery", "getCounter()", Some(U256::from(1))),
    ] {
        let section = compiled
            .split(&format!(":{contract} ======="))
            .nth(1)
            .unwrap_or_else(|| panic!("solc output missing contract {contract}"));
        let hex = section
            .lines()
            .skip_while(|line| *line != "Binary:")
            .nth(1)
            .unwrap_or_else(|| panic!("solc output missing bytecode for {contract}"));
        let bytecode = Bytes::from_str(&format!("0x{hex}")).expect("invalid contract bytecode");
        let mut db = StateDb::new(EvmState::default());
        let deployed = {
            let mut evm = factory.create_evm(&mut db, EvmEnv::default());
            evm.transact(ArcologyTx::from(TxEnv {
                caller,
                kind: TxKind::Create,
                data: bytecode,
                gas_limit: 20_000_000,
                gas_price: 0,
                ..Default::default()
            }))
            .expect("deployment transaction failed")
        };
        assert!(
            deployed.result.is_success(),
            "{contract} deployment: {:?}",
            deployed.result
        );
        let address = deployed
            .result
            .created_address()
            .expect("deployment returned no address");
        db.commit(deployed.state);

        let result = {
            let mut evm = factory.create_evm(&mut db, EvmEnv::default());
            evm.transact(ArcologyTx::from(TxEnv {
                caller,
                kind: TxKind::Call(address),
                data: keccak256(method.as_bytes())[..4].to_vec().into(),
                gas_limit: 20_000_000,
                gas_price: 0,
                nonce: 1,
                ..Default::default()
            }))
            .expect("contract transaction failed")
        };
        assert!(
            result.result.is_success(),
            "{contract}.{method}: {:?}",
            result.result
        );
        if let Some(expected) = expected {
            assert_eq!(
                result
                    .result
                    .output()
                    .expect("contract returned no output")
                    .as_ref(),
                expected.to_be_bytes::<32>(),
                "{contract}.{method}"
            );
        }
        println!("PASS {contract}.{method}");
    }
}
