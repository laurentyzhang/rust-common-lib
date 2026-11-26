use super::*;

#[test]
fn u256_round_trip() {
    for value in [U256::ZERO, U256::from(1), U256::MAX] {
        assert_eq!(U256::from_word(value.to_word()), Some(value));
    }
}

#[test]
fn i256_round_trip() {
    for value in [I256::MIN, I256::MINUS_ONE, I256::ZERO, I256::ONE, I256::MAX] {
        assert_eq!(I256::from_word(value.to_word()), Some(value));
    }
}
