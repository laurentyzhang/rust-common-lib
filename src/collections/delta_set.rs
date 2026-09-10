/*
*   Copyright (c) 2026 Arcology Network

*   This program is free software: you can redistribute it and/or modify
*   it under the terms of the GNU General Public License as published by
*   the Free Software Foundation, either version 3 of the License, or
*   (at your option) any later version.

*   This program is distributed in the hope that it will be useful,
*   but WITHOUT ANY WARRANTY; without even the implied warranty of
*   MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
*   GNU General Public License for more details.

*   You should have received a copy of the GNU General Public License
*   along with this program.  If not, see <https://www.gnu.org/licenses/>.
*/

use im::HashMap;

#[derive(Clone, Default)]
pub struct DeltaSet<K> {
    elements: Vec<Option<K>>,
    keys: im::HashMap<K, u64>,
    committed: im::HashMap<K, u64>,
    sequence: u64,
}

impl<K> PartialEq for DeltaSet<K>
where
    K: Eq + std::hash::Hash,
{
    fn eq(&self, other: &Self) -> bool {
        self.elements == other.elements
            && self.keys == other.keys
            && self.committed == other.committed
            && self.sequence == other.sequence
    }
}

impl<K> DeltaSet<K>
where
    K: Eq + std::hash::Hash,
{
    /// Creates an empty set with space reserved for at least `reserved` element slots.
    pub fn new(reserved: u64) -> Self {
        DeltaSet {
            elements: Vec::with_capacity(
                usize::try_from(reserved).expect("reserved capacity exceeds usize::MAX"),
            ),
            keys: HashMap::new(),
            committed: HashMap::new(),
            sequence: 0,
        }
    }

    /// Rebuilds a clean, compact set from unique live elements in insertion order.
    ///
    /// Returns `None` if the input contains duplicates or cannot be indexed by `u64`.
    pub(crate) fn try_from_elements(elements: Vec<K>) -> Option<Self>
    where
        K: Clone,
    {
        let sequence = u64::try_from(elements.len()).ok()?;
        let mut keys = HashMap::new();

        for (index, key) in elements.iter().cloned().enumerate() {
            let index = u64::try_from(index).ok()?;
            if keys.insert(key, index).is_some() {
                return None;
            }
        }

        Some(Self {
            elements: elements.into_iter().map(Some).collect(),
            committed: keys.clone(),
            keys,
            sequence,
        })
    }

    /// Rebuilds a clean set while preserving empty slots and stable indexes.
    pub(crate) fn try_from_slots(elements: Vec<Option<K>>) -> Option<Self>
    where
        K: Clone,
    {
        let sequence = u64::try_from(elements.len()).ok()?;
        let mut keys = HashMap::new();

        for (index, key) in elements.iter().enumerate() {
            if let Some(key) = key {
                let index = u64::try_from(index).ok()?;
                if keys.insert(key.clone(), index).is_some() {
                    return None;
                }
            }
        }

        Some(Self {
            elements,
            committed: keys.clone(),
            keys,
            sequence,
        })
    }

    /// Returns all allocated slots, including slots left empty by removals.
    pub(crate) fn slots(&self) -> &[Option<K>] {
        &self.elements
    }
    /// Returns whether the live keys differ from the last committed snapshot.
    fn is_dirty(&self) -> bool {
        self.keys != self.committed
    }

    /// Iterates over live elements in insertion order, skipping removed slots.
    pub(crate) fn iter_live(&self) -> impl Iterator<Item = &K> {
        self.elements.iter().flatten()
    }

    /// Returns the live element equal to `key`, or `None` if it is absent.
    pub fn get(&self, key: &K) -> Option<&K> {
        let ind = usize::try_from(*self.keys.get(key)?).ok()?;
        self.elements.get(ind)?.as_ref()
    }

    /// Returns the live element at `ind`, or `None` for a removed or invalid slot.
    pub fn get_at(&self, ind: u64) -> Option<&K> {
        let ind = usize::try_from(ind).ok()?;
        self.elements.get(ind)?.as_ref()
    }

    /// Inserts a new element and returns its stable slot index.
    ///
    /// Returns `None` if an equal live element is already present.
    pub fn insert(&mut self, key: K) -> Option<u64>
    where
        K: Clone,
    {
        if self.keys.contains_key(&key) {
            return None;
        }

        self.elements.push(Some(key.clone()));
        self.keys.insert(key, self.sequence);
        self.sequence += 1;
        Some(self.sequence - 1)
    }

    /// Removes `key`, leaving an empty slot, and returns the element and its index.
    pub fn remove(&mut self, key: &K) -> Option<(K, u64)>
    where
        K: Eq + std::hash::Hash + Clone,
    {
        let ind = usize::try_from(*self.keys.get(key)?).ok()?;
        self.elements[ind].take();
        self.keys.remove_with_key(key)
    }

    /// Removes the live element at `ind`, leaving an empty slot.
    pub fn remove_at(&mut self, ind: u64) -> Option<(K, u64)>
    where
        K: Clone,
    {
        let element_index = usize::try_from(ind).ok()?;
        if element_index >= self.elements.len() {
            return None;
        }

        let key = self.elements[element_index].take()?;
        self.keys.remove_with_key(&key)
    }

    /// Returns the number of allocated slots, including slots left by removals.
    pub fn len(&self) -> u64 {
        u64::try_from(self.elements.len()).expect("element count exceeds u64::MAX")
    }

    /// Returns `(added, removed)` keys relative to the committed snapshot.
    ///
    /// The order of keys in either vector is unspecified.
    pub fn diff(&self) -> (Vec<K>, Vec<K>)
    where
        K: Clone,
    {
        let added = self
            .keys
            .clone()
            .relative_complement(self.committed.clone());

        let removed = self
            .committed
            .clone()
            .relative_complement(self.keys.clone());

        let new_keys: Vec<K> = added.into_iter().map(|(key, _)| key).collect();
        let removed_keys: Vec<K> = removed.into_iter().map(|(key, _)| key).collect();
        (new_keys, removed_keys)
    }

    /// Applies removals followed by additions to the live set.
    ///
    /// Missing removals and duplicate additions are ignored.
    pub fn apply_delta(&mut self, removed: &[K], added: &[K])
    where
        K: Clone,
    {
        for k in removed {
            self.remove(k);
        }

        for k in added {
            self.insert(k.clone());
        }
    }

    /// Marks the current live keys as committed.
    pub(crate) fn commit(&mut self)
    where
        K: Clone,
    {
        self.committed = self.keys.clone();
    }
    /// Removes empty slots and rebuilds the key-to-index map.
    ///
    /// Returns the number of live elements reindexed after the first removed slot.
    /// Existing indexes at and after that slot are invalidated.
    pub fn compact(&mut self) -> u64
    where
        K: Clone + std::fmt::Debug,
    {
        let Some(first_index) = self.elements.iter().position(Option::is_none) else {
            return 0;
        };

        self.elements.retain(Option::is_some);

        // Rebuild the key-to-index map after removing empty slots.
        self.elements
            .iter()
            .flatten()
            .enumerate()
            .for_each(|(index, key)| {
                self.keys
                    .get_mut(key)
                    .map(|ind| *ind = u64::try_from(index).expect("element index exceeds u64::MAX"))
                    .unwrap_or_else(|| panic!("missing key: {:?}", key));
            });
        self.sequence = u64::try_from(self.elements.len()).expect("element count exceeds u64::MAX");
        u64::try_from(self.elements.len() - first_index)
            .expect("compacted element count exceeds u64::MAX")
    }
}

#[cfg(test)]
mod tests {
    use super::DeltaSet;

    fn key(value: &str) -> String {
        value.to_owned()
    }

    #[test]
    fn insert_and_get_by_key_and_index() {
        let mut set = DeltaSet::new(4);

        assert_eq!(set.insert(key("alpha")), Some(0));
        assert_eq!(set.insert(key("beta")), Some(1));

        assert_eq!(set.get(&key("alpha")).map(String::as_str), Some("alpha"));
        assert_eq!(set.get_at(1).map(String::as_str), Some("beta"));
        assert_eq!(set.get_at(2), None);
    }

    #[test]
    fn duplicate_insert_is_rejected() {
        let mut set = DeltaSet::new(0);

        assert_eq!(set.insert(key("alpha")), Some(0));
        assert_eq!(set.insert(key("alpha")), None);
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn remove_by_key_and_index_clears_elements() {
        let mut set = DeltaSet::new(0);
        set.insert(key("alpha"));
        set.insert(key("beta"));

        assert_eq!(set.remove(&key("alpha")), Some((key("alpha"), 0)));
        assert_eq!(set.get(&key("alpha")), None);
        assert_eq!(set.get_at(0), None);

        assert_eq!(set.remove_at(1), Some((key("beta"), 1)));
        assert_eq!(set.get(&key("beta")), None);
        assert_eq!(set.get_at(1), None);
    }

    #[test]
    fn diff_reports_added_and_removed_keys() {
        let mut set = DeltaSet::new(0);
        set.insert(key("alpha"));
        set.insert(key("beta"));
        set.committed = set.keys.clone();

        set.remove(&key("alpha"));
        set.insert(key("gamma"));

        let (mut added, mut removed) = set.diff();
        added.sort();
        removed.sort();

        assert_eq!(added, vec![key("gamma")]);
        assert_eq!(removed, vec![key("alpha")]);
    }

    #[test]
    fn apply_delta_keeps_elements_and_keys_consistent() {
        let mut set = DeltaSet::new(0);
        set.insert(key("alpha"));
        set.insert(key("beta"));

        set.apply_delta(&[key("alpha")], &[key("gamma")]);

        assert_eq!(set.get(&key("alpha")), None);
        assert_eq!(set.get_at(0), None);
        assert_eq!(set.get(&key("gamma")).map(String::as_str), Some("gamma"));
    }

    #[test]
    fn compact_removes_holes_and_reindexes_remaining_keys() {
        let mut set = DeltaSet::new(0);
        set.insert(key("alpha"));
        set.insert(key("beta"));
        set.insert(key("gamma"));
        set.remove(&key("beta"));

        set.compact();

        assert_eq!(set.get_at(0).map(String::as_str), Some("alpha"));
        assert_eq!(set.get_at(1).map(String::as_str), Some("gamma"));
        assert_eq!(set.get(&key("gamma")).map(String::as_str), Some("gamma"));
        assert_eq!(set.insert(key("delta")), Some(2));
        assert_eq!(set.get(&key("delta")).map(String::as_str), Some("delta"));
    }
}

#[cfg(test)]
mod convergence_tests {
    use super::DeltaSet;

    fn key(value: &str) -> String {
        value.to_owned()
    }

    fn values(set: &DeltaSet<String>) -> Vec<String> {
        let mut values: Vec<_> = set.elements.iter().flatten().cloned().collect();
        values.sort();
        values
    }

    fn commit(set: &mut DeltaSet<String>) {
        set.committed = set.keys.clone();
    }

    #[test]
    fn three_sets_converge_after_add_and_remove_deltas() {
        let mut a = DeltaSet::new(0);
        let mut b = DeltaSet::new(0);
        let mut c = DeltaSet::new(0);

        assert!(a.committed.is_empty());
        assert!(b.committed.is_empty());
        assert!(c.committed.is_empty());

        for value in ["one", "two", "three"] {
            a.insert(key(value));
        }
        for value in ["three", "four", "five"] {
            b.insert(key(value));
        }
        for value in ["five", "six", "one"] {
            c.insert(key(value));
        }

        let (a_added, a_removed) = a.diff();
        let (b_added, b_removed) = b.diff();
        let (c_added, c_removed) = c.diff();

        for set in [&mut a, &mut b, &mut c] {
            set.apply_delta(&a_removed, &a_added);
            set.apply_delta(&b_removed, &b_added);
            set.apply_delta(&c_removed, &c_added);
        }

        assert_eq!(values(&a), values(&b));
        assert_eq!(values(&b), values(&c));

        commit(&mut a);
        commit(&mut b);
        commit(&mut c);

        for value in ["one", "two"] {
            a.remove(&key(value));
        }
        for value in ["two", "three"] {
            b.remove(&key(value));
        }
        for value in ["three", "four"] {
            c.remove(&key(value));
        }

        let (a_added, a_removed) = a.diff();
        let (b_added, b_removed) = b.diff();
        let (c_added, c_removed) = c.diff();

        for set in [&mut a, &mut b, &mut c] {
            set.apply_delta(&a_removed, &a_added);
            set.apply_delta(&b_removed, &b_added);
            set.apply_delta(&c_removed, &c_added);
        }

        assert_eq!(values(&a), values(&b));
        assert_eq!(values(&b), values(&c));
        assert_eq!(values(&a), vec![key("five"), key("six")]);
    }
}
