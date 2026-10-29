use super::super::state::SlotStorage;
use super::{Field, ScalarSlots};
use crate::crdt::{self, state::DeltaOp};
use crate::execution::evm::{ApiAddress, KeyCodec};
use alloy_evm::revm::state::EvmStorage;
use alloy_primitives::{Address, I256, U256};

#[test]
fn u256_values_and_full_width_deltas_round_trip() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = ScalarSlots::new(key);
    let mut storage = EvmStorage::default();
    for (value, delta) in [
        (U256::ZERO, Some(DeltaOp::Add(U256::MAX))),
        (U256::MAX, Some(DeltaOp::Sub(U256::MAX))),
        (U256::from(42), None),
    ] {
        let state = crdt::Scalar::<U256> {
            value,
            delta,
            ..Default::default()
        };
        slots.store(&mut storage, owner, &state).unwrap();
        assert_eq!(
            slots.load::<U256, _>(&mut storage, owner).unwrap(),
            (state, slots)
        );
        assert_eq!(
            ScalarSlots::read_min::<U256, _>(&mut storage, owner, key),
            Some(U256::ZERO)
        );
        assert_eq!(
            ScalarSlots::read_max::<U256, _>(&mut storage, owner, key),
            Some(U256::MAX)
        );
        assert_eq!(
            ScalarSlots::read_value::<U256, _>(&mut storage, owner, key),
            Some(value)
        );
    }
}

#[test]
fn i256_values_use_twos_complement_and_unsigned_delta_magnitudes() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = ScalarSlots::new(key);
    let mut storage = EvmStorage::default();
    for (value, delta) in [
        (I256::MIN, Some(DeltaOp::Add(U256::MAX))),
        (I256::MAX, Some(DeltaOp::Sub(U256::MAX))),
        (I256::unchecked_from(-50), Some(DeltaOp::Add(U256::from(7)))),
        (I256::ZERO, Some(DeltaOp::Sub(U256::from(1)))),
        (I256::unchecked_from(-1), None),
    ] {
        let state = crdt::Scalar::<I256> {
            value,
            delta,
            ..Default::default()
        };
        slots.store(&mut storage, owner, &state).unwrap();
        assert_eq!(storage.read(owner, slots.value).unwrap(), value.into_raw());
        assert_eq!(
            slots.load::<I256, _>(&mut storage, owner).unwrap(),
            (state, slots)
        );
        assert_eq!(
            ScalarSlots::read_min::<I256, _>(&mut storage, owner, key),
            Some(I256::MIN)
        );
        assert_eq!(
            ScalarSlots::read_max::<I256, _>(&mut storage, owner, key),
            Some(I256::MAX)
        );
        assert_eq!(
            ScalarSlots::read_value::<I256, _>(&mut storage, owner, key),
            Some(value)
        );
    }
}

#[test]
fn typed_loads_accept_full_width_values_and_magnitudes() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = ScalarSlots::new(key);
    let mut storage = EvmStorage::default();
    slots
        .store(&mut storage, owner, &crdt::Scalar::<U256>::default())
        .unwrap();
    storage.write(owner, slots.value, U256::MAX).unwrap();
    assert_eq!(
        slots.load::<U256, _>(&mut storage, owner).unwrap().0.value,
        U256::MAX
    );
    assert_eq!(
        ScalarSlots::read_value::<U256, _>(&mut storage, owner, key),
        Some(U256::MAX)
    );

    slots
        .store(&mut storage, owner, &crdt::Scalar::<I256>::default())
        .unwrap();
    for value in [I256::MIN, I256::MAX] {
        storage.write(owner, slots.value, value.into_raw()).unwrap();
        assert_eq!(
            slots.load::<I256, _>(&mut storage, owner).unwrap().0.value,
            value
        );
        assert_eq!(
            ScalarSlots::read_value::<I256, _>(&mut storage, owner, key),
            Some(value)
        );
    }

    storage.write(owner, slots.value, U256::ZERO).unwrap();
    storage.write(owner, slots.pos_delta, U256::MAX).unwrap();
    assert_eq!(
        slots.load::<I256, _>(&mut storage, owner).unwrap().0.delta,
        Some(DeltaOp::Add(U256::MAX))
    );
}

#[test]
fn split_deltas_round_trip_and_clear_the_opposite_slot() {
    let owner = Address::ZERO;
    let key = KeyCodec::encode(ApiAddress::Scalar, Field::Value as u8, Address::ZERO);
    let slots = ScalarSlots::new(key);
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
            ScalarSlots::read_delta(&mut storage, owner, key),
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
    let slots = ScalarSlots::new(key);
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
                ScalarSlots::read_delta(&mut storage, owner, field_key),
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
        assert_eq!(ScalarSlots::read_delta(&mut storage, owner, invalid), None);
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
            ScalarSlots::new(key)
                .store(&mut storage, Address::ZERO, &state)
                .is_err()
        );
        assert!(storage.is_empty());
    }
}
