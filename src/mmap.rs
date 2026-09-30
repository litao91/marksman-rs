//! Ordered multi-map: one key to a set of values.
//!
//! Port of `Marksman.MMap`.

use std::collections::{BTreeMap, BTreeSet};

use crate::misc::Difference;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MMap<K: Ord, V: Ord>(pub BTreeMap<K, BTreeSet<V>>);

impl<K: Ord, V: Ord> Default for MMap<K, V> {
    fn default() -> Self {
        MMap(BTreeMap::new())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MMapDifference<K: Ord, V: Ord> {
    pub removed_keys: BTreeSet<K>,
    pub added_keys: BTreeSet<K>,
    pub changed_keys: Vec<(K, Difference<V>)>,
}

impl<K: Ord, V: Ord> MMapDifference<K, V> {
    pub fn is_empty(&self) -> bool {
        self.removed_keys.is_empty() && self.added_keys.is_empty() && self.changed_keys.is_empty()
    }
}

impl<K: Ord + std::fmt::Display, V: Ord + Clone + std::fmt::Display> MMapDifference<K, V> {
    pub fn compact_format(&self) -> String {
        let mut lines: Vec<String> = Vec::new();

        if !self.removed_keys.is_empty() {
            lines.push("Removed keys:".to_string());
            for key in &self.removed_keys {
                lines.push(crate::misc::indented(4, &key.to_string()));
            }
        }

        if !self.added_keys.is_empty() {
            lines.push("Added keys:".to_string());
            for key in &self.added_keys {
                lines.push(crate::misc::indented(4, &key.to_string()));
            }
        }

        if !self.changed_keys.is_empty() {
            lines.push("Changed keys:".to_string());
            for (key, diff) in &self.changed_keys {
                lines.push(crate::misc::indented(4, &key.to_string()));
                lines.push(crate::misc::indented(4, &diff.compact_format()));
            }
        }

        lines.join("\n")
    }
}

impl<K: Ord + Clone, V: Ord + Clone> MMap<K, V> {
    pub fn empty() -> MMap<K, V> {
        MMap(BTreeMap::new())
    }

    pub fn inner(&self) -> &BTreeMap<K, BTreeSet<V>> {
        &self.0
    }

    pub fn try_find(&self, k: &K) -> Option<&BTreeSet<V>> {
        self.0.get(k)
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.0.contains_key(k)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn add(&self, k: K, v: V) -> MMap<K, V> {
        let mut map = self.0.clone();
        map.entry(k).or_default().insert(v);
        MMap(map)
    }

    /// In-place variants. Callers that own the map use these so that a single
    /// update costs O(log n) instead of copying the whole index.
    pub fn add_mut(&mut self, k: K, v: V) {
        self.0.entry(k).or_default().insert(v);
    }

    pub fn remove_value_mut(&mut self, k: &K, v: &V) {
        if let Some(set) = self.0.get_mut(k) {
            set.remove(v);
            if set.is_empty() {
                self.0.remove(k);
            }
        }
    }

    pub fn set_values_mut(&mut self, k: K, values: BTreeSet<V>) {
        if values.is_empty() {
            self.0.remove(&k);
        } else {
            self.0.insert(k, values);
        }
    }

    pub fn remove_key_mut(&mut self, k: &K) {
        self.0.remove(k);
    }

    pub fn add_empty(&self, k: K) -> MMap<K, V> {
        let mut map = self.0.clone();
        map.entry(k).or_default();
        MMap(map)
    }

    pub fn remove_value(&self, k: K, v: V) -> MMap<K, V> {
        let mut map = self.0.clone();
        if let Some(set) = map.get_mut(&k) {
            set.remove(&v);
            if set.is_empty() {
                map.remove(&k);
            }
        }
        MMap(map)
    }

    pub fn set_values(&self, k: K, values: BTreeSet<V>) -> MMap<K, V> {
        let mut map = self.0.clone();
        if values.is_empty() {
            map.remove(&k);
        } else {
            map.insert(k, values);
        }
        MMap(map)
    }

    pub fn remove_key(&self, k: &K) -> MMap<K, V> {
        let mut map = self.0.clone();
        map.remove(k);
        MMap(map)
    }

    pub fn of_seq(seq: impl IntoIterator<Item = (K, V)>) -> MMap<K, V> {
        let mut map: BTreeMap<K, BTreeSet<V>> = BTreeMap::new();
        for (k, v) in seq {
            map.entry(k).or_default().insert(v);
        }
        MMap(map)
    }

    pub fn to_seq(&self) -> impl Iterator<Item = (K, V)> + '_ {
        self.0.iter().flat_map(|(k, vs)| vs.iter().map(move |v| (k.clone(), v.clone())))
    }

    pub fn to_set_seq(&self) -> impl Iterator<Item = (&K, &BTreeSet<V>)> {
        self.0.iter()
    }

    pub fn map_keys<K2: Ord + Clone>(&self, f: impl Fn(&K) -> K2) -> MMap<K2, V> {
        let mut m = BTreeMap::new();
        for (k, vs) in &self.0 {
            m.insert(f(k), vs.clone());
        }
        MMap(m)
    }

    pub fn iter(&self, mut f: impl FnMut(&K, &V)) {
        for (k, vs) in &self.0 {
            for v in vs {
                f(k, v);
            }
        }
    }

    pub fn fold<T>(&self, state: T, mut f: impl FnMut(T, &K, &V) -> T) -> T {
        let mut acc = state;
        for (k, vs) in &self.0 {
            for v in vs {
                acc = f(acc, k, v);
            }
        }
        acc
    }

    pub fn difference(m1: &MMap<K, V>, m2: &MMap<K, V>) -> MMapDifference<K, V> {
        let ks1: BTreeSet<&K> = m1.0.keys().collect();
        let ks2: BTreeSet<&K> = m2.0.keys().collect();

        let same: BTreeSet<&K> = ks1.intersection(&ks2).copied().collect();
        let removed_keys: BTreeSet<K> = ks1.difference(&same).map(|k| (*k).clone()).collect();
        let added_keys: BTreeSet<K> = ks2.difference(&same).map(|k| (*k).clone()).collect();

        let mut changed_keys = Vec::new();
        for k in same {
            let s1 = &m1.0[k];
            let s2 = &m2.0[k];
            let diff = Difference::mk(s1, s2);
            if !diff.is_empty() {
                changed_keys.push((k.clone(), diff));
            }
        }

        MMapDifference { removed_keys, added_keys, changed_keys }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_groups_values_under_key() {
        let m = MMap::empty().add("a", 1).add("a", 2).add("b", 3);
        assert_eq!(
            m.try_find(&"a").unwrap().iter().copied().collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(
            m.try_find(&"b").unwrap().iter().copied().collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(m.try_find(&"missing"), None);
    }

    #[test]
    fn removing_last_value_drops_the_key() {
        let m = MMap::empty().add("a", 1);
        assert!(m.remove_value("a", 1).is_empty());
    }

    #[test]
    fn difference_reports_added_removed_and_changed() {
        let before = MMap::empty().add("a", 1).add("b", 2);
        let after = MMap::empty().add("a", 3).add("c", 4);
        let diff = MMap::difference(&before, &after);
        assert_eq!(diff.removed_keys.iter().copied().collect::<Vec<_>>(), vec!["b"]);
        assert_eq!(diff.added_keys.iter().copied().collect::<Vec<_>>(), vec!["c"]);
        assert_eq!(diff.changed_keys.len(), 1);
    }
}
