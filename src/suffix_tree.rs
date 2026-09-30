//! A trie keyed on the *reversed* components of a document path.
//!
//! Port of `Marksman.SuffixTree`. Reversing the key means a subtree collects every
//! document whose path ends with a given suffix, which is how a wiki-link like
//! `[[note]]` finds `dir/sub/note.md`.

use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Node<V: Ord + Clone> {
    nodes: BTreeMap<String, Node<V>>,
    values: BTreeSet<V>,
}

impl<V: Ord + Clone> Node<V> {
    fn empty() -> Node<V> {
        Node { nodes: BTreeMap::new(), values: BTreeSet::new() }
    }

    fn has_children(&self) -> bool {
        !self.nodes.is_empty()
    }

    fn has_value(&self) -> bool {
        !self.values.is_empty()
    }

    fn has_data(&self) -> bool {
        self.has_value() || self.has_children()
    }

    fn add(&self, key_rev: &[String], value: V) -> Node<V> {
        let mut out = self.clone();
        out.add_mut(key_rev, value);
        out
    }

    fn add_mut(&mut self, key_rev: &[String], value: V) {
        match key_rev {
            [] => {
                self.values.insert(value);
            }
            [head, tail @ ..] => {
                self.nodes.entry(head.clone()).or_insert_with(Node::empty).add_mut(tail, value);
            }
        }
    }

    fn remove_where(
        &self,
        key_rev: &[String],
        remove_values: &mut dyn FnMut(BTreeSet<V>) -> BTreeSet<V>,
    ) -> Node<V> {
        fn go<V2: Ord + Clone>(
            node: &Node<V2>,
            key_rev: &[String],
            remove_values: &mut dyn FnMut(BTreeSet<V2>) -> BTreeSet<V2>,
        ) -> Option<Node<V2>> {
            match key_rev {
                [] => {
                    let mut updated = node.clone();
                    updated.values = remove_values(node.values.clone());
                    if updated.has_data() {
                        Some(updated)
                    } else {
                        None
                    }
                }
                [head, tail @ ..] => match node.nodes.get(head) {
                    None => Some(node.clone()),
                    Some(tl_tree) => match go(tl_tree, tail, remove_values) {
                        Some(new_tl) => {
                            let mut out = node.clone();
                            out.nodes.insert(head.clone(), new_tl);
                            Some(out)
                        }
                        None => {
                            let mut out = node.clone();
                            out.nodes.remove(head);
                            if out.has_data() {
                                Some(out)
                            } else {
                                None
                            }
                        }
                    },
                },
            }
        }

        go(self, key_rev, remove_values).unwrap_or_else(Node::empty)
    }

    fn find_subtree<'a>(&'a self, key_rev: &[String]) -> Option<&'a Node<V>> {
        match key_rev {
            [] => Some(self),
            [head, tail @ ..] => self.nodes.get(head).and_then(|sub| sub.find_subtree(tail)),
        }
    }

    fn collect_values(&self, out: &mut Vec<V>) {
        out.extend(self.values.iter().cloned());
        for sub in self.nodes.values() {
            sub.collect_values(out);
        }
    }
}

/// Maps keys of type `K` into a reversed-component trie via `split_fn`.
///
/// No `PartialEq`: `split_fn` is a function pointer, and comparing those says
/// nothing about whether two trees hold the same values.
#[derive(Clone, Debug)]
pub struct SuffixTree<K, V: Ord + Clone> {
    split_fn: fn(&K) -> Vec<String>,
    tree: Node<V>,
    _key: std::marker::PhantomData<K>,
}

impl<K, V: Ord + Clone> SuffixTree<K, V> {
    pub fn empty(split_fn: fn(&K) -> Vec<String>) -> SuffixTree<K, V> {
        SuffixTree { split_fn, tree: Node::empty(), _key: std::marker::PhantomData }
    }

    fn rev(&self, k: &K) -> Vec<String> {
        let mut parts = (self.split_fn)(k);
        parts.reverse();
        parts
    }

    pub fn of_seq(split_fn: fn(&K) -> Vec<String>, data: impl IntoIterator<Item = (K, V)>) -> SuffixTree<K, V> {
        let mut tree = Node::empty();
        for (k, v) in data {
            let mut parts = split_fn(&k);
            parts.reverse();
            tree.add_mut(&parts, v);
        }
        SuffixTree { split_fn, tree, _key: std::marker::PhantomData }
    }

    pub fn add(&self, k: &K, v: V) -> SuffixTree<K, V> {
        SuffixTree {
            split_fn: self.split_fn,
            tree: self.tree.add(&self.rev(k), v),
            _key: std::marker::PhantomData,
        }
    }

    pub fn remove(&self, k: &K) -> SuffixTree<K, V> {
        SuffixTree {
            split_fn: self.split_fn,
            tree: self.tree.remove_where(&self.rev(k), &mut |_| BTreeSet::new()),
            _key: std::marker::PhantomData,
        }
    }

    pub fn remove_value(&self, k: &K, v: V) -> SuffixTree<K, V> {
        SuffixTree {
            split_fn: self.split_fn,
            tree: self.tree.remove_where(&self.rev(k), &mut move |mut set| {
                set.remove(&v);
                set
            }),
            _key: std::marker::PhantomData,
        }
    }

    /// Every value whose key ends with `k`'s components.
    pub fn filter_matching_values(&self, k: &K) -> Vec<V> {
        self.filter_matching_parts(&self.rev(k))
    }

    pub fn find_exact_values(&self, k: &K) -> BTreeSet<V> {
        match self.tree.find_subtree(&self.rev(k)) {
            Some(sub) => sub.values.clone(),
            None => BTreeSet::new(),
        }
    }

    /// Query with key components that have already been split and reversed.
    pub fn filter_matching_parts(&self, parts_rev: &[String]) -> Vec<V> {
        match self.tree.find_subtree(parts_rev) {
            Some(sub) => {
                let mut out = Vec::new();
                sub.collect_values(&mut out);
                out
            }
            None => Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        !self.tree.has_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(s: &String) -> Vec<String> {
        s.split('/').filter(|c| !c.is_empty()).map(|c| c.to_string()).collect()
    }

    fn tree() -> SuffixTree<String, i32> {
        SuffixTree::of_seq(
            split,
            vec![
                ("a/b/note".to_string(), 1),
                ("c/note".to_string(), 2),
                ("a/other".to_string(), 3),
            ],
        )
    }

    #[test]
    fn suffix_match_collects_all_documents_with_that_path_ending() {
        assert_eq!(tree().filter_matching_values(&"note".to_string()), vec![1, 2]);
        assert_eq!(tree().filter_matching_values(&"b/note".to_string()), vec![1]);
        assert_eq!(tree().filter_matching_values(&"missing".to_string()), Vec::<i32>::new());
    }

    #[test]
    fn exact_match_only_returns_values_at_that_node() {
        assert_eq!(
            tree().find_exact_values(&"a/b/note".to_string()).iter().copied().collect::<Vec<_>>(),
            vec![1]
        );
        assert!(tree().find_exact_values(&"note".to_string()).is_empty());
    }

    #[test]
    fn add_and_remove_value_prune_empty_branches() {
        let t = tree().add(&"d/note".to_string(), 4);
        assert_eq!(t.filter_matching_values(&"note".to_string()), vec![1, 2, 4]);
        let t = t.remove_value(&"d/note".to_string(), 4);
        assert_eq!(t.filter_matching_values(&"note".to_string()), vec![1, 2]);
        assert!(t.remove(&"d/note".to_string()).filter_matching_values(&"d/note".to_string()).is_empty());
    }
}
