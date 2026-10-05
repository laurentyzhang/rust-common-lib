use alloy_evm::{
    Evm, EvmEnv, EvmFactory,
    revm::{context::TxEnv, database::EmptyDB},
};
use alloy_primitives::{Bytes, TxKind};
use rust_common_lib::evm::{
    ArcologyApi, ArcologyEvmFactory, ArcologyTx, CallHookInput, CallHookOutput, CallHookRouter,
    ClosureCallHook, arcology_address,
};

fn main() {
    let mut router = CallHookRouter::new();
    router
        .register(
            ArcologyApi::Runtime,
            ClosureCallHook::new(|input: &CallHookInput<'_>| {
                println!(
                    "Arcology runtime call: caller={:?}, calldata=0x{}",
                    input.caller,
                    alloy_primitives::hex::encode(input.input)
                );
                Ok(CallHookOutput::returned(
                    100,
                    Bytes::from_static(b"handled by Arcology"),
                ))
            }),
        )
        .unwrap();

    let factory = ArcologyEvmFactory::try_new(router).unwrap();
    let mut evm = factory.create_evm(EmptyDB::default(), EvmEnv::default());
    let result = evm
        .transact(ArcologyTx::from(TxEnv {
            caller: arcology_address(0x11),
            gas_limit: 100_000,
            gas_price: 0,
            kind: TxKind::Call(ArcologyApi::Runtime.address()),
            data: Bytes::from_static(b"pid()"),
            ..Default::default()
        }))
        .unwrap();

    println!("EVM output: {:?}", result.result.output());
}
