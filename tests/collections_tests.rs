//! Ports of `Tests/MMapTests.fs`, `Tests/PartitionedMapTests.fs` and
//! `Tests/SuffixTreeTests.fs`.

mod common;

use std::collections::{BTreeMap, BTreeSet};

use marksman::mmap::MMap;
use marksman::partitioned_map::PartitionedMap;
use marksman::suffix_tree::SuffixTree;

/// Reproduces `new System.Random(seed)` from .NET. Seeded `Random` instances
/// still use the legacy Knuth subtractive generator in .NET 6 and later, which
/// is what the original tests rely on for their edit sequences.
struct DotNetRandom {
    seed_array: [i32; 56],
    inext: usize,
    inext_p: usize,
}

impl DotNetRandom {
    const MBIG: i32 = i32::MAX;
    const MSEED: i32 = 161803398;

    fn new(seed: i32) -> DotNetRandom {
        let mut seed_array = [0i32; 56];
        let subtraction = if seed == i32::MIN { i32::MAX } else { seed.abs() };
        let mut mj = DotNetRandom::MSEED - subtraction;
        seed_array[55] = mj;
        let mut mk = 1;
        for i in 1..55usize {
            let ii = (21 * i) % 55;
            seed_array[ii] = mk;
            mk = mj - mk;
            if mk < 0 {
                mk += DotNetRandom::MBIG;
            }
            mj = seed_array[ii];
        }
        for _k in 1..5 {
            for i in 1..56usize {
                seed_array[i] -= seed_array[1 + ((i + 30) % 55)];
                if seed_array[i] < 0 {
                    seed_array[i] += DotNetRandom::MBIG;
                }
            }
        }
        DotNetRandom { seed_array, inext: 0, inext_p: 21 }
    }

    fn sample(&mut self) -> f64 {
        let mut loc_inext = self.inext;
        let mut loc_inext_p = self.inext_p;

        loc_inext += 1;
        if loc_inext >= 56 {
            loc_inext = 1;
        }
        loc_inext_p += 1;
        if loc_inext_p >= 56 {
            loc_inext_p = 1;
        }

        let mut ret_val = self.seed_array[loc_inext] - self.seed_array[loc_inext_p];
        if ret_val == DotNetRandom::MBIG {
            ret_val -= 1;
        }
        if ret_val < 0 {
            ret_val += DotNetRandom::MBIG;
        }

        self.seed_array[loc_inext] = ret_val;
        self.inext = loc_inext;
        self.inext_p = loc_inext_p;

        ret_val as f64 * (1.0 / DotNetRandom::MBIG as f64)
    }

    /// `Random.Next(maxValue)`, i.e. a value in `[0, maxValue)`.
    fn next(&mut self, max_value: i32) -> i32 {
        (self.sample() * max_value as f64) as i32
    }
}

// ---------------------------------------------------------------------------
// Marksman.MMapTests / DifferenceTests
// ---------------------------------------------------------------------------

mod difference_tests {
    use super::*;

    /// `MMapTests.DifferenceTests.keyIntersection`
    #[test]
    fn key_intersection() {
        let m1 = MMap::of_seq([("a", 1)]);
        let m2 = MMap::of_seq([("b", 2)]);
        let diff = MMap::difference(&m1, &m2);
        assert!(diff.added_keys.contains("b"));
        assert!(diff.removed_keys.contains("a"));
        assert!(diff.changed_keys.is_empty());
    }
}

// ---------------------------------------------------------------------------
// Marksman.PartitionedMapTests
// ---------------------------------------------------------------------------

/// `PartitionedMapTests.differencesMatchOrdinaryMapsAcrossSnapshots`
#[test]
fn differences_match_ordinary_maps_across_snapshots() {
    let mut random = DotNetRandom::new(42);
    let mut expected: BTreeMap<i32, i32> = BTreeMap::new();
    let mut actual: PartitionedMap<i32, i32> = PartitionedMap::empty();
    let mut snapshots: Vec<(BTreeMap<i32, i32>, PartitionedMap<i32, i32>)> = Vec::new();
    snapshots.push((expected.clone(), actual.clone()));

    for step in 1..=300 {
        let key = random.next(200);

        if random.next(3) == 0 {
            expected.remove(&key);
            actual.remove_mut(&key);
        } else {
            let value = random.next(20);
            expected.insert(key, value);
            actual.add_mut(key, value);
        }

        if step % 50 == 0 {
            snapshots.push((expected.clone(), actual.clone()));
            let actual_seq: BTreeMap<i32, i32> = actual.to_seq().collect();
            assert_eq!(expected, actual_seq);
        }
    }

    for (old_map, old_parts) in &snapshots {
        for (new_map, new_parts) in &snapshots {
            let keys: BTreeSet<i32> = old_map.keys().chain(new_map.keys()).copied().collect();

            let expected_changes: BTreeSet<(i32, Option<i32>, Option<i32>)> = keys
                .iter()
                .filter_map(|key| {
                    let before = old_map.get(key).copied();
                    let after = new_map.get(key).copied();
                    if before == after {
                        None
                    } else {
                        Some((*key, before, after))
                    }
                })
                .collect();

            let mut actual_changes: BTreeSet<(i32, Option<i32>, Option<i32>)> = BTreeSet::new();

            PartitionedMap::iter_differences(old_parts, new_parts, |key, before, after| {
                actual_changes.insert((*key, before.copied(), after.copied()));
            });

            assert_eq!(expected_changes, actual_changes);
        }
    }

    // The F# original asserts `obj.ReferenceEquals(withValue, unchanged)`, i.e.
    // re-inserting an identical value is a no-op that hands back the very same
    // map. The Rust port exposes that as `add_mut` reporting "unchanged".
    let mut with_value = actual.clone();
    assert!(with_value.add_mut(42, 123));
    assert!(!with_value.add_mut(42, 123));
}

// ---------------------------------------------------------------------------
// Marksman.SuffixTreeTests / ImplTests
// ---------------------------------------------------------------------------

mod impl_tests {
    use super::*;

    type Tree = SuffixTree<Vec<String>, i32>;

    /// The original tests exercise `SuffixTree.Impl` directly, whose keys are
    /// already `list<string>`. `Node` is private in the Rust port, so the same
    /// tree is built through the public `SuffixTree` with an identity split
    /// function.
    fn components(key: &Vec<String>) -> Vec<String> {
        key.clone()
    }

    fn k(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|p| p.to_string()).collect()
    }

    fn tree() -> Tree {
        SuffixTree::of_seq(
            components,
            vec![
                (k(&["a", "b"]), 0),
                (k(&["a", "b", "c"]), 1),
                (k(&["a", "b", "d"]), 2),
                (k(&["b"]), 3),
                (k(&["c"]), 4),
            ],
        )
    }

    /// Every value in the tree, in the order `collectValues` walks it. The Rust
    /// port has no public `collect_values`; querying with an empty key walks the
    /// root and produces exactly the same sequence.
    fn collect_values(tree: &Tree) -> Vec<i32> {
        tree.filter_matching_values(&Vec::new())
    }

    /// `SuffixTreeTests.ImplTests.filterTest`
    #[test]
    fn filter_test() {
        let tree = tree();

        let actual = tree.filter_matching_values(&k(&["c"]));
        assert_eq!(vec![4, 1], actual);

        let actual = tree.filter_matching_values(&k(&["d"]));
        assert_eq!(vec![2], actual);

        let actual = tree.filter_matching_values(&k(&["b", "c"]));
        assert_eq!(vec![1], actual);

        let actual = tree.filter_matching_values(&k(&["e"]));
        assert_eq!(Vec::<i32>::new(), actual);

        let actual = tree.filter_matching_values(&k(&["b", "z"]));
        assert_eq!(Vec::<i32>::new(), actual);
    }

    /// `SuffixTreeTests.ImplTests.removeTest`
    #[test]
    fn remove_test() {
        let tree = tree();

        let actual = collect_values(&tree.remove(&k(&["a", "b", "c"])));
        assert_eq!(vec![3, 0, 4, 2], actual);

        let actual = collect_values(&tree.remove(&k(&["a", "b", "z"])));
        assert_eq!(vec![3, 0, 4, 1, 2], actual);

        let actual = collect_values(&tree.remove(&k(&["c"])));
        assert_eq!(vec![3, 0, 1, 2], actual);

        let actual = collect_values(&tree.remove(&k(&["a", "b"])));
        assert_eq!(vec![3, 4, 1, 2], actual);
    }

    /// `SuffixTreeTests.ImplTests.multipleValuesAtTheSamePathRemainIndependent`
    #[test]
    fn multiple_values_at_the_same_path_remain_independent() {
        let tree: Tree =
            SuffixTree::of_seq(components, vec![(k(&["notes"]), 1), (k(&["notes"]), 2)]);

        let exact: Vec<i32> = tree.find_exact_values(&k(&["notes"])).into_iter().collect();
        assert_eq!(vec![1, 2], exact);

        let remaining = tree.remove_value(&k(&["notes"]), 1);
        let exact: Vec<i32> =
            remaining.find_exact_values(&k(&["notes"])).into_iter().collect();
        assert_eq!(vec![2], exact);
    }
}
