//! Undirected graph over an ordered multi-map.
//!
//! Port of `Marksman.Graph`.

use std::collections::BTreeSet;

use crate::misc::Difference;
use crate::mmap::MMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Graph<V: Ord + Clone> {
    pub edges: MMap<V, V>,
}

#[derive(Clone, Debug)]
pub struct GraphDifference<V: Ord + Clone> {
    pub edge_difference: Difference<(V, V)>,
}

impl<V: Ord + Clone> GraphDifference<V> {
    pub fn is_empty(&self) -> bool {
        self.edge_difference.is_empty()
    }
}

impl<V: Ord + Clone> GraphDifference<V> {
    /// `edge` renders one edge. Tuples have no `Display` of their own, so the
    /// caller supplies the rendering rather than the bound.
    pub fn compact_format_with(&self, edge: impl Fn(&V, &V) -> String) -> String {
        if self.is_empty() {
            return String::new();
        }

        let mut lines = vec!["Edge difference:".to_string()];

        if !self.edge_difference.added.is_empty() {
            lines.push("Added:".to_string());
            for (src, dst) in &self.edge_difference.added {
                lines.push(crate::misc::indented(4, &edge(src, dst)));
            }
        }

        if !self.edge_difference.removed.is_empty() {
            lines.push("Removed:".to_string());
            for (src, dst) in &self.edge_difference.removed {
                lines.push(crate::misc::indented(4, &edge(src, dst)));
            }
        }

        lines.join("\n")
    }
}

impl<V: Ord + Clone> Graph<V> {
    pub fn empty() -> Graph<V> {
        Graph { edges: MMap::empty() }
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    pub fn has_vertex(&self, v: &V) -> bool {
        self.edges.contains_key(v)
    }

    pub fn degree(&self, v: &V) -> Option<usize> {
        self.edges.try_find(v).map(|s| s.len())
    }

    pub fn is_degree_0_or_absent(&self, v: &V) -> bool {
        self.degree(v).unwrap_or(0) == 0
    }

    pub fn edges_of(&self, v: &V) -> BTreeSet<V> {
        self.edges.try_find(v).cloned().unwrap_or_default()
    }

    pub fn add_edge(&self, src: V, dest: V) -> Graph<V> {
        let edges = self
            .edges
            .clone()
            .add(src.clone(), dest.clone())
            .add(dest, src);
        Graph { edges }
    }

    pub fn remove_edge(&self, src: V, dest: V) -> Graph<V> {
        let edges = self
            .edges
            .clone()
            .remove_value(src.clone(), dest.clone())
            .remove_value(dest, src);
        Graph { edges }
    }

    pub fn remove_vertex_with_callback(
        &self,
        cb: &mut impl FnMut(&V, &Graph<V>) -> Graph<V>,
        v: V,
    ) -> Graph<V> {
        let neighbors = self.edges_of(&v);
        let mut g = self.clone();
        for dest in neighbors {
            g = cb(&dest, &g);
            g = g.remove_edge(v.clone(), dest.clone());
            g = g.remove_edge(dest, v.clone());
        }
        g
    }

    pub fn remove_vertex(&self, v: V) -> Graph<V> {
        self.remove_vertex_with_callback(&mut |_, g| g.clone(), v)
    }

    pub fn difference(g1: &Graph<V>, g2: &Graph<V>) -> GraphDifference<V> {
        let edges1: BTreeSet<(V, V)> = g1.edges.to_seq().collect();
        let edges2: BTreeSet<(V, V)> = g2.edges.to_seq().collect();
        GraphDifference { edge_difference: Difference::mk(&edges1, &edges2) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edges_are_symmetric() {
        let g = Graph::empty().add_edge(1, 2);
        assert_eq!(g.edges_of(&1).iter().copied().collect::<Vec<_>>(), vec![2]);
        assert_eq!(g.edges_of(&2).iter().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!(g.degree(&1), Some(1));
        assert!(g.has_vertex(&1));
        assert!(!g.has_vertex(&3));
    }

    #[test]
    fn removing_vertex_clears_both_ends() {
        let g = Graph::empty().add_edge(1, 2);
        let g = g.remove_vertex(1);
        assert!(g.is_degree_0_or_absent(&2));
    }

    #[test]
    fn difference_renders_nested_and_indented() {
        let before = Graph::empty().add_edge(1, 2);
        let after = Graph::empty().add_edge(1, 3);
        let diff = Graph::difference(&before, &after);

        // Edges are symmetric, so both directions of each change are reported.
        assert_eq!(
            diff.compact_format_with(|s, d| format!("({s}, {d})")),
            "Edge difference:\nAdded:\n    (1, 3)\n    (3, 1)\nRemoved:\n    (1, 2)\n    (2, 1)"
        );
    }
}
