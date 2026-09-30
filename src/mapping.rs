//! A many-to-one mapping with an inverse index.
//!
//! Port of `Marksman.Mapping`. Used to relate concrete syntax elements to
//! abstract elements and abstract elements to symbols.

use std::collections::{BTreeMap, BTreeSet};

use crate::mmap::MMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mapping<Dom: Ord + Clone, Cod: Ord + Clone> {
    mapping: BTreeMap<Dom, Cod>,
    inverse: MMap<Cod, Dom>,
}

impl<Dom: Ord + Clone, Cod: Ord + Clone> Default for Mapping<Dom, Cod> {
    fn default() -> Self {
        Mapping::empty()
    }
}

impl<Dom: Ord + Clone, Cod: Ord + Clone> Mapping<Dom, Cod> {
    pub fn empty() -> Mapping<Dom, Cod> {
        Mapping { mapping: BTreeMap::new(), inverse: MMap::empty() }
    }

    pub fn add(&self, x: Dom, y: Cod) -> Mapping<Dom, Cod> {
        let mut out = self.clone();
        out.add_mut(x, y);
        out
    }

    /// Building a mapping element by element must not copy it each time.
    pub fn add_mut(&mut self, x: Dom, y: Cod) {
        if let Some(old_cod) = self.mapping.get(&x).cloned() {
            self.inverse.remove_value_mut(&old_cod, &x);
        }
        self.mapping.insert(x.clone(), y.clone());
        self.inverse.add_mut(y, x);
    }

    pub fn in_dom(&self, x: &Dom) -> bool {
        self.mapping.contains_key(x)
    }

    pub fn in_cod(&self, y: &Cod) -> bool {
        self.inverse.contains_key(y)
    }

    pub fn image(&self, x: &Dom) -> Option<&Cod> {
        self.mapping.get(x)
    }

    pub fn pre_image(&self, y: &Cod) -> BTreeSet<Dom> {
        self.inverse.try_find(y).cloned().unwrap_or_default()
    }

    pub fn try_pre_image(&self, y: &Cod) -> Option<BTreeSet<Dom>> {
        self.inverse.try_find(y).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_and_pre_image_agree() {
        let m = Mapping::empty().add(1, "a").add(2, "a").add(3, "b");
        assert_eq!(m.image(&1), Some(&"a"));
        assert_eq!(m.pre_image(&"a").iter().copied().collect::<Vec<_>>(), vec![1, 2]);
        assert!(m.in_cod(&"b"));
        assert!(!m.in_dom(&4));
    }

    #[test]
    fn remapping_a_domain_element_drops_the_old_inverse_entry() {
        let m = Mapping::empty().add(1, "a").add(1, "b");
        assert!(m.pre_image(&"a").is_empty());
        assert_eq!(m.pre_image(&"b").iter().copied().collect::<Vec<_>>(), vec![1]);
    }
}
