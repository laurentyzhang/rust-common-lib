use super::ShardMap;

#[test]
fn apply_batch_updates_values_in_shards() {
    let mut map = ShardMap::<u64, u64>::new(4);
    let updates = vec![(1, 10), (2, 20), (1, 11), (3, 30), (4, 40), (5, 50)];

    map.apply_batch(updates);

    assert_eq!(map.get(&1), Some(&11));
    assert_eq!(map.get(&2), Some(&20));
    assert_eq!(map.get(&3), Some(&30));
    assert_eq!(map.get(&4), Some(&40));
    assert_eq!(map.get(&5), Some(&50));
    assert_eq!(map.contains_key(&6), false);
}
