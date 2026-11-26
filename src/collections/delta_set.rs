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
    first_hole: Option<u64>, // Lowest index left empty by a removal.
}

impl<K: std::fmt::Debug + std::hash::Hash + Eq> std::fmt::Debug for DeltaSet<K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeltaSet")
            .field("elements", &self.elements)
            .field("keys", &self.keys)
            .field("committed", &self.committed)
            .field("sequence", &self.sequence)
            .field("first_hole", &self.first_hole)
            .finish()
    }
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
            first_hole: None,
        }
    }

    /// Rebuilds a clean set while preserving empty slots and stable indexes.
    pub(crate) fn try_from_slots(elements: Vec<Option<K>>) -> Option<Self>
    where
        K: Clone,
    {
        let sequence = u64::try_from(elements.len()).ok()?;
        let first_hole = elements.iter().position(Option::is_none).map(|i| i as u64);
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
            first_hole,
        })
    }

    /// Returns all allocated slots, including slots left empty by removals.
    #[cfg(test)]
    pub(crate) fn slots(&self) -> &[Option<K>] {
        &self.elements
    }
    /// Returns whether the live keys differ from the last committed snapshot.
    // fn is_dirty(&self) -> bool {
    //     self.keys != self.committed
    // }

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
        self.mark_hole(ind as u64);
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
        self.mark_hole(ind);
        self.keys.remove_with_key(&key)
    }

    /// Lowers the first-hole marker to `ind` if it is earlier.
    fn mark_hole(&mut self, ind: u64) {
        self.first_hole = Some(self.first_hole.map_or(ind, |hole| hole.min(ind)));
    }

    /// Returns the index span `0..span()`, including holes left by removals.
    pub fn span(&self) -> u64 {
        u64::try_from(self.elements.len()).expect("element count exceeds u64::MAX")
    }

    /// Returns the number of live elements, excluding holes left by removals.
    pub fn len(&self) -> u64 {
        u64::try_from(self.keys.len()).expect("key count exceeds u64::MAX")
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

    /// Records the current live keys as the committed snapshot.
    pub(crate) fn snapshot(&mut self)
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
        let Some(first_index) = self.first_hole.take() else {
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
        self.sequence - first_index
    }
}

#[cfg(test)]
#[path = "tests/delta_set.rs"]
mod tests;

#[cfg(test)]
#[path = "tests/delta_set_convergence.rs"]
mod convergence_tests;
