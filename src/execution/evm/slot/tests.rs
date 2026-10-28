use super::{Field, SlotKeys, SlotStorage};
use crate::crdt::{self, state::DeltaOp};
use crate::execution::evm::{ApiAddress, KeyCodec};
use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::{Address, I256, U256};

#[test]
fn u64_values_and_full_width_deltas_round_trip() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = SlotKeys::new(key);
    let mut storage = EvmStorage::default();
    for (value, delta) in [
        (0, Some(DeltaOp::Add(u64::MAX))),
        (u64::MAX, Some(DeltaOp::Sub(u64::MAX))),
        (42, None),
    ] {
        let state = crdt::Scalar::<u64> {
            value,
            delta,
            ..Default::default()
        };
        slots.store(&mut storage, owner, &state).unwrap();
        assert_eq!(
            slots.load::<u64, _>(&mut storage, owner).unwrap(),
            (state, slots)
        );
        assert_eq!(
            SlotKeys::read_min::<u64, _>(&mut storage, owner, key),
            Some(0)
        );
        assert_eq!(
            SlotKeys::read_max::<u64, _>(&mut storage, owner, key),
            Some(u64::MAX)
        );
        assert_eq!(
            SlotKeys::read_value::<u64, _>(&mut storage, owner, key),
            Some(value)
        );
    }
}

#[test]
fn i64_values_use_sign_extension_and_unsigned_delta_magnitudes() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = SlotKeys::new(key);
    let mut storage = EvmStorage::default();
    for (value, delta) in [
        (i64::MIN, Some(DeltaOp::Add(u64::MAX))),
        (i64::MAX, Some(DeltaOp::Sub(u64::MAX))),
        (-50, Some(DeltaOp::Add(7))),
        (0, Some(DeltaOp::Sub(1))),
        (-1, None),
    ] {
        let state = crdt::Scalar::<i64> {
            value,
            delta,
            ..Default::default()
        };
        slots.store(&mut storage, owner, &state).unwrap();
        assert_eq!(
            storage.read(owner, slots.value).unwrap(),
            I256::unchecked_from(value).into_raw()
        );
        assert_eq!(
            slots.load::<i64, _>(&mut storage, owner).unwrap(),
            (state, slots)
        );
        assert_eq!(
            SlotKeys::read_min::<i64, _>(&mut storage, owner, key),
            Some(i64::MIN)
        );
        assert_eq!(
            SlotKeys::read_max::<i64, _>(&mut storage, owner, key),
            Some(i64::MAX)
        );
        assert_eq!(
            SlotKeys::read_value::<i64, _>(&mut storage, owner, key),
            Some(value)
        );
    }
}

#[test]
fn typed_loads_reject_out_of_range_values_and_magnitudes() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = SlotKeys::new(key);
    let mut storage = EvmStorage::default();
    slots
        .store(&mut storage, owner, &crdt::Scalar::<u64>::default())
        .unwrap();
    storage
        .write(owner, slots.value, U256::from(u64::MAX) + U256::from(1))
        .unwrap();
    assert!(slots.load::<u64, _>(&mut storage, owner).is_err());
    assert_eq!(
        SlotKeys::read_value::<u64, _>(&mut storage, owner, key),
        None
    );

    slots
        .store(&mut storage, owner, &crdt::Scalar::<i64>::default())
        .unwrap();
    for value in [
        U256::from(i64::MAX as u64) + U256::from(1),
        I256::unchecked_from(i64::MIN).into_raw() - U256::from(1),
    ] {
        storage.write(owner, slots.value, value).unwrap();
        assert!(slots.load::<i64, _>(&mut storage, owner).is_err());
        assert_eq!(
            SlotKeys::read_value::<i64, _>(&mut storage, owner, key),
            None
        );
    }

    storage.write(owner, slots.value, U256::ZERO).unwrap();
    storage
        .write(owner, slots.pos_delta, U256::from(u64::MAX) + U256::from(1))
        .unwrap();
    assert!(slots.load::<i64, _>(&mut storage, owner).is_err());
}

#[test]
fn split_deltas_round_trip_and_clear_the_opposite_slot() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = SlotKeys::new(key);
    let mut storage = EvmStorage::default();
    for (value, delta, expected) in [
        (
            U256::ZERO,
            Some(DeltaOp::Add(U256::MAX)),
            (U256::MAX, U256::ZERO),
        ),
        (
            U256::MAX,
            Some(DeltaOp::Sub(U256::MAX)),
            (U256::ZERO, U256::MAX),
        ),
        (
            U256::from(50),
            Some(DeltaOp::Add(U256::from(7))),
            (U256::from(7), U256::ZERO),
        ),
        (
            U256::from(50),
            Some(DeltaOp::Sub(U256::from(7))),
            (U256::ZERO, U256::from(7)),
        ),
        (U256::from(50), None, (U256::ZERO, U256::ZERO)),
    ] {
        let state = crdt::Scalar::<U256> {
            value,
            delta,
            ..Default::default()
        };
        slots.store(&mut storage, owner, &state).unwrap();
        assert_eq!(
            SlotKeys::read_delta(&mut storage, owner, key),
            Some(expected)
        );
        assert_eq!(
            slots.load::<U256, _>(&mut storage, owner).unwrap(),
            (state, slots)
        );
    }
}

#[test]
fn load_combines_both_magnitudes_and_read_delta_preserves_them() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = SlotKeys::new(key);
    let mut storage = EvmStorage::default();
    let state = crdt::Scalar::<U256> {
        value: U256::from(50),
        ..Default::default()
    };
    slots.store(&mut storage, owner, &state).unwrap();
    for (pos, neg, expected) in [
        (
            U256::from(10),
            U256::from(3),
            Some(DeltaOp::Add(U256::from(7))),
        ),
        (
            U256::from(3),
            U256::from(10),
            Some(DeltaOp::Sub(U256::from(7))),
        ),
        (U256::MAX, U256::MAX, None),
    ] {
        storage.write(owner, slots.pos_delta, pos).unwrap();
        storage.write(owner, slots.neg_delta, neg).unwrap();
        for field_key in [
            slots.min,
            slots.max,
            slots.value,
            slots.pos_delta,
            slots.neg_delta,
        ] {
            assert_eq!(
                SlotKeys::read_delta(&mut storage, owner, field_key),
                Some((pos, neg))
            );
        }
        assert_eq!(
            slots.load::<U256, _>(&mut storage, owner).unwrap().0.delta,
            expected
        );
    }
    for invalid in [
        U256::ZERO,
        KeyCodec::encode(ApiAddress::Scalar, 5, Address::ZERO),
        KeyCodec::encode(ApiAddress::Container, 3, Address::ZERO),
    ] {
        assert_eq!(SlotKeys::read_delta(&mut storage, owner, invalid), None);
    }
}

#[test]
fn invalid_pending_arithmetic_does_not_write_any_slots() {
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    for (value, delta) in [
        (U256::MAX, DeltaOp::Add(U256::from(1))),
        (U256::ZERO, DeltaOp::Sub(U256::from(1))),
    ] {
        let mut storage = EvmStorage::default();
        let state = crdt::Scalar::<U256> {
            value,
            delta: Some(delta),
            ..Default::default()
        };
        assert!(
            SlotKeys::new(key)
                .store(&mut storage, Address::ZERO, &state)
                .is_err()
        );
        assert!(storage.is_empty());
    }
}
