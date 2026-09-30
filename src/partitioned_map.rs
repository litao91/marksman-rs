//! An immutable map split into stable partitions.
//!
//! Port of `Marksman.PartitionedMap`. Updating one key preserves the identity of
//! every other partition, so two snapshots can compare only the partitions that
//! differ. Partitions are `Arc`-shared to make that identity check cheap.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::misc::{sorted_merge_iter, MergeEntry};

/// Updating copies 64 map roots; a changed partition then contains about N/64
/// values if hashes are spread evenly. This beat 16 roots at 1,000 documents.
const PARTITION_COUNT: usize = 64;

#[derive(Clone, Debug)]
pub struct PartitionedMap<K: Ord, V: PartialEq> {
    partitions: Vec<Arc<BTreeMap<K, V>>>,
}

impl<K: Ord + Clone, V: PartialEq + Clone> PartitionedMap<K, V> {
    pub fn new() -> PartitionedMap<K, V> {
        Self::empty()
    }

    pub fn empty() -> PartitionedMap<K, V> {
        PartitionedMap {
            partitions: (0..PARTITION_COUNT).map(|_| Arc::new(BTreeMap::new())).collect(),
        }
    }

    fn partition(key: &K) -> usize
    where
        K: Hash,
    {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hasher);
        (hasher.finish() as usize) & (PARTITION_COUNT - 1)
    }

    pub fn try_find(&self, key: &K) -> Option<&V>
    where
        K: Hash,
    {
        self.partitions[Self::partition(key)].get(key)
    }

    pub fn contains_key(&self, key: &K) -> bool
    where
        K: Hash,
    {
        self.partitions[Self::partition(key)].contains_key(key)
    }

    /// Inserts in place, returning whether the stored value actually changed.
    ///
    /// Partitions are `Arc`-shared with any earlier snapshot, so `make_mut`
    /// copies only the one partition being touched, and only while a snapshot
    /// still holds it. A map that is being built for the first time therefore
    /// never copies at all, while two snapshots still differ by identity in
    /// exactly the partitions that changed.
    pub fn add_mut(&mut self, key: K, value: V) -> bool
    where
        K: Hash,
        V: Clone,
    {
        let index = Self::partition(&key);

        // Checked before `make_mut` so that a no-op leaves a shared partition
        // alone; copying it would break the identity `iter_differences` uses.
        if self.partitions[index].get(&key) == Some(&value) {
            return false;
        }

        Arc::make_mut(&mut self.partitions[index]).insert(key, value);
        true
    }

    pub fn remove_mut(&mut self, key: &K) -> bool
    where
        K: Hash,
        V: Clone,
    {
        let index = Self::partition(key);

        if !self.partitions[index].contains_key(key) {
            return false;
        }

        Arc::make_mut(&mut self.partitions[index]).remove(key);
        true
    }

    pub fn to_seq(&self) -> impl Iterator<Item = (K, V)> + '_ {
        self.partitions
            .iter()
            .flat_map(|p| p.iter().map(|(k, v)| (k.clone(), v.clone())))
    }

    pub fn iter(&self, mut visit: impl FnMut(&K, &V)) {
        for partition in &self.partitions {
            for (k, v) in partition.iter() {
                visit(k, v);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.partitions.iter().map(|p| p.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.partitions.iter().all(|p| p.is_empty())
    }

    /// Visits keys whose values differ, including additions and removals.
    /// Unchanged partitions are skipped by identity; changed partitions are
    /// compared exactly, so unrelated hash collisions cannot change results.
    pub fn iter_differences(
        before: &PartitionedMap<K, V>,
        after: &PartitionedMap<K, V>,
        mut visit: impl FnMut(&K, Option<&V>, Option<&V>),
    ) {
        for index in 0..PARTITION_COUNT {
            let old_partition = &before.partitions[index];
            let new_partition = &after.partitions[index];

            if Arc::ptr_eq(old_partition, new_partition) {
                continue;
            }

            let old_seq = old_partition.iter().map(|(k, v)| (k.clone(), v));
            let new_seq = new_partition.iter().map(|(k, v)| (k.clone(), v));

            sorted_merge_iter(old_seq, new_seq, |key, entry| match entry {
                MergeEntry::OnlyBefore(value) => visit(key, Some(value), None),
                MergeEntry::OnlyAfter(value) => visit(key, None, Some(value)),
                MergeEntry::Both(old_value, new_value) => {
                    if old_value != new_value {
                        visit(key, Some(old_value), Some(new_value));
                    }
                }
            });
        }
    }
}

impl<K: Ord + Clone + Hash, V: PartialEq + Clone> PartialEq for PartitionedMap<K, V> {
    fn eq(&self, other: &Self) -> bool {
        for index in 0..PARTITION_COUNT {
            if Arc::ptr_eq(&self.partitions[index], &other.partitions[index]) {
                continue;
            }
            if self.partitions[index] != other.partitions[index] {
                return false;
            }
        }
        true
    }
}

impl<K: Ord + Clone + Hash, V: PartialEq + Clone> Eq for PartitionedMap<K, V> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_insert_is_reported() {
        let mut map: PartitionedMap<String, i32> = PartitionedMap::empty();
        assert!(map.add_mut("k".to_string(), 1));
        assert!(!map.add_mut("k".to_string(), 1));
        assert!(map.add_mut("k".to_string(), 2));
        assert_eq!(map.try_find(&"k".to_string()), Some(&2));
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn differences_only_visit_changed_keys() {
        let mut before: PartitionedMap<String, i32> = PartitionedMap::empty();
        before.add_mut("a".to_string(), 1);
        before.add_mut("b".to_string(), 2);

        let mut after = before.clone();
        after.add_mut("b".to_string(), 3);
        after.add_mut("c".to_string(), 4);

        let mut seen = Vec::new();
        PartitionedMap::iter_differences(&before, &after, |k, old, new| {
            seen.push((k.clone(), old.copied(), new.copied()))
        });
        seen.sort();

        assert_eq!(
            seen,
            vec![
                ("b".to_string(), Some(2), Some(3)),
                ("c".to_string(), None, Some(4))
            ]
        );
    }

    #[test]
    fn removal_is_reported_as_a_difference() {
        let mut before: PartitionedMap<String, i32> = PartitionedMap::empty();
        before.add_mut("a".to_string(), 1);
        let mut after = before.clone();
        assert!(after.remove_mut(&"a".to_string()));

        let mut seen = Vec::new();
        PartitionedMap::iter_differences(&before, &after, |k, old, new| {
            seen.push((k.clone(), old.copied(), new.copied()))
        });
        assert_eq!(seen, vec![("a".to_string(), Some(1), None)]);
    }

    #[test]
    fn a_no_op_write_keeps_every_partition_identical() {
        let mut before: PartitionedMap<u32, u32> = PartitionedMap::empty();
        for i in 0..1000u32 {
            before.add_mut(i, i);
        }

        let mut after = before.clone();
        assert!(!after.add_mut(0, 0));
        assert!(!after.remove_mut(&5000));

        // The original asserts `obj.ReferenceEquals` on the map itself; here the
        // property that matters is that no partition was copied.
        assert!(after
            .partitions
            .iter()
            .zip(&before.partitions)
            .all(|(new, old)| Arc::ptr_eq(new, old)));
    }

    #[test]
    fn untouched_partitions_keep_their_identity_across_snapshots() {
        let mut before: PartitionedMap<u32, u32> = PartitionedMap::empty();
        for i in 0..1000u32 {
            before.add_mut(i, i);
        }
        let mut after = before.clone();
        after.add_mut(0, 9999);

        // Only the partition holding key 0 differs, so the diff visits one key.
        let mut seen = Vec::new();
        PartitionedMap::iter_differences(&before, &after, |k, _, _| seen.push(*k));
        assert_eq!(seen, vec![0]);

        // The snapshot was not disturbed by the later insert.
        assert_eq!(before.try_find(&0), Some(&0));
        assert_eq!(after.try_find(&0), Some(&9999));
    }
}
