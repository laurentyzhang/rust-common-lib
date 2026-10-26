use alloy_evm::{
    Evm, EvmEnv, EvmFactory,
    revm::{context::TxEnv, database::EmptyDB},
};
use alloy_primitives::{Address, Bytes, TxKind};
use rust_common_lib::execution::evm::{
    ApiAddress, ArcologyEvmFactory, ArcologyTx, CallInput, CallOutput, CallRouter,
};

fn main() {
    let mut router = CallRouter::new();
    router
        .register(ApiAddress::Runtime.address(), |input: &CallInput<'_>| {
            println!(
                "Arcology runtime call: caller={:?}, calldata=0x{}",
                input.caller,
                alloy_primitives::hex::encode(input.input)
            );
            Ok(CallOutput::returned(
                100,
                Bytes::from_static(b"handled by Arcology"),
            ))
        })
        .unwrap();

    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: Address::with_last_byte(0x11),
            gas_limit: 100_000,
            gas_price: 0,
            kind: TxKind::Call(ApiAddress::Runtime.address()),
            data: Bytes::from_static(b"pid()"),
            ..Default::default()
        }))
        .unwrap();

    println!("EVM output: {:?}", result.result.output());
}
