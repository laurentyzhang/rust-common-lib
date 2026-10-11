#[cfg(test)]
mod tests {
    use crate::crdt::crdt::{CacheableCrdt, Crdt};
    use crate::crdt::types::NonComm;
    use std::cmp::Ordering;

    #[test]
    fn default_has_no_value_or_delta() {
        let bytes = NonComm::<[u8]>::default();

        assert_eq!(bytes.value(), None);
        assert_eq!(bytes.delta.as_deref(), None);
        assert_eq!(bytes.limits(), None);
        assert!(!bytes.is_numeric());
        assert!(!bytes.is_commutative());
    }

    #[test]
    fn delta_is_the_current_value() {
        let mut bytes = NonComm::<[u8]>::default();

        bytes.add_delta(&[1, 2, 3]).expect("add delta");

        assert_eq!(bytes.value(), Some(&[1, 2, 3][..]));
        assert_eq!(bytes.delta.as_deref(), Some(&[1, 2, 3][..]));
    }

    #[test]
    fn setting_delta_again_replaces_the_value() {
        let mut bytes = NonComm::<[u8]>::default();
        bytes.add_delta(&[1, 2, 3]).expect("add delta");

        bytes.add_delta(&[4, 5]).expect("replace delta");

        assert_eq!(bytes.value(), Some(&[4, 5][..]));
        assert_eq!(bytes.delta.as_deref(), Some(&[4, 5][..]));
    }

    #[test]
    fn apply_delta_is_a_no_op() {
        let mut bytes = NonComm::<[u8]>::default();
        bytes.add_delta(&[1, 2, 3]).expect("add delta");

        assert_eq!(bytes.apply_delta().value(), Some(&[1, 2, 3][..]));
        assert_eq!(bytes.delta.as_deref(), Some(&[1, 2, 3][..]));
    }

    #[test]
    fn empty_delta_is_preserved() {
        let mut bytes = NonComm::<[u8]>::default();
        bytes.add_delta(&[]).expect("add empty delta");

        assert_eq!(bytes.value(), Some(&[][..]));
        assert_eq!(bytes.delta.as_deref(), Some(&[][..]));
    }

    #[test]
    fn cache_weight_counts_struct_and_payload_bytes() {
        let mut bytes = NonComm::<[u8]>::default();
        let base = std::mem::size_of::<NonComm<[u8]>>();
        assert_eq!(bytes.cache_weight(), base);

        bytes.add_delta(&[1, 2, 3]).expect("add delta");
        assert_eq!(bytes.cache_weight(), base + 3);

        bytes.add_delta(&[4, 5]).expect("replace delta");
        assert_eq!(bytes.cache_weight(), base + 2);
    }

    #[test]
    fn compare_uses_length_then_lexicographic_order() {
        let short = NonComm::<[u8]>::new(vec![b'z']).unwrap();
        let long = NonComm::<[u8]>::new(vec![b'a', b'a']).unwrap();
        let left = NonComm::<[u8]>::new(vec![b'a', b'b']).unwrap();
        let right = NonComm::<[u8]>::new(vec![b'a', b'c']).unwrap();
        let no_delta = NonComm::<[u8]>::default();

        assert_eq!(short.compare(&long), Ordering::Less);
        assert_eq!(left.compare(&right), Ordering::Less);
        assert_eq!(right.compare(&left), Ordering::Greater);
        assert_eq!(left.compare(&left), Ordering::Equal);
        assert_eq!(left.compare(&no_delta), Ordering::Less);
        assert_eq!(no_delta.compare(&left), Ordering::Greater);
        assert_eq!(no_delta.compare(&no_delta), Ordering::Equal);
    }
}
