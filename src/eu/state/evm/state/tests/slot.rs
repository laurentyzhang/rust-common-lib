use super::*;

#[test]
fn reads_and_writes_present_values() {
    let owner = Address::repeat_byte(0x11);
    let slot = U256::from(7);
    let mut state = StateDb::default();

    assert_eq!(state.read(owner, slot).unwrap(), U256::ZERO);
    state.write(owner, slot, U256::from(10)).unwrap();
    state.write(owner, slot, U256::from(20)).unwrap();
    assert_eq!(state.read(owner, slot).unwrap(), U256::from(20));

    let account = state.inner().get(&owner).unwrap();
    assert!(account.is_touched());
    let stored = account.storage.get(&slot).unwrap();
    assert_eq!(stored.original_value(), U256::ZERO);
    assert_eq!(stored.present_value(), U256::from(20));
}
