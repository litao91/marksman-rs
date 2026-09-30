//! Per-document lookup tables over the concrete syntax tree.
//!
//! Port of `Marksman.Index`.

use std::collections::BTreeMap;

use lsp_types::Position;

use crate::cst::{self, Element, Heading, MdLink, MdLinkDef, Node, Tag, TextNode, WikiLink};
use crate::misc::{LinkLabel, Slug};

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub titles: Vec<Node<Heading>>,
    pub headings: Vec<Node<Heading>>,
    pub headings_by_slug: BTreeMap<Slug, Vec<Node<Heading>>>,
    pub wiki_links: Vec<Node<WikiLink>>,
    pub md_links: Vec<Node<MdLink>>,
    pub link_defs: Vec<Node<MdLinkDef>>,
    pub tags: Vec<Node<Tag>>,
    pub yaml_front_matter: Option<TextNode>,
}

impl Index {
    pub fn of_cst(cels: &[Element]) -> Index {
        let mut titles = Vec::new();
        let mut headings_by_slug: BTreeMap<Slug, Vec<Node<Heading>>> = BTreeMap::new();
        let mut wiki_links = Vec::new();
        let mut headings = Vec::new();
        let mut md_links = Vec::new();
        let mut link_defs = Vec::new();
        let mut tags = Vec::new();
        let mut yaml = None;

        for el in cels {
            match el {
                Element::H(hn) => {
                    let slug = hn.data.slug();
                    headings_by_slug.entry(slug).or_default().push(hn.clone());
                    if hn.data.is_title() {
                        titles.push(hn.clone());
                    }
                    headings.push(hn.clone());
                }
                Element::WL(wl) => wiki_links.push(wl.clone()),
                Element::ML(ml) => md_links.push(ml.clone()),
                Element::MLD(link_def) => link_defs.push(link_def.clone()),
                Element::T(t) => tags.push(t.clone()),
                Element::YML(yml) => yaml = Some(yml.clone()),
            }
        }

        Index {
            titles,
            headings,
            headings_by_slug,
            wiki_links,
            md_links,
            link_defs,
            tags,
            yaml_front_matter: yaml,
        }
    }

    pub fn title(&self) -> Option<&Node<Heading>> {
        self.titles.first()
    }

    pub fn links(&self) -> impl Iterator<Item = Element> + '_ {
        self.wiki_links
            .iter()
            .map(|n| Element::WL(n.clone()))
            .chain(self.md_links.iter().map(|n| Element::ML(n.clone())))
    }

    pub fn filter_tags_by_name(&self, name: &str) -> Vec<&Node<Tag>> {
        self.tags.iter().filter(|t| t.data.name.text == name).collect()
    }

    pub fn try_find_link_def(&self, label: &LinkLabel) -> Option<&Node<MdLinkDef>> {
        self.link_defs
            .iter()
            .find(|ld| &ld.data.normalized_label() == label)
    }

    pub fn filter_link_defs(&self, pred: impl Fn(&LinkLabel) -> bool) -> Vec<&Node<MdLinkDef>> {
        self.link_defs
            .iter()
            .filter(|ld| pred(&ld.data.normalized_label()))
            .collect()
    }

    pub fn filter_heading_by_slug(&self, slug: &Slug) -> Vec<Node<Heading>> {
        self.headings_by_slug.get(slug).cloned().unwrap_or_default()
    }

    pub fn link_at_pos(&self, pos: Position) -> Option<Element> {
        let matching = |range: lsp_types::Range| range.start <= pos && pos < range.end;

        self.wiki_links
            .iter()
            .find(|n| matching(n.range))
            .map(|n| Element::WL(n.clone()))
            .or_else(|| {
                self.md_links
                    .iter()
                    .find(|n| matching(n.range))
                    .map(|n| Element::ML(n.clone()))
            })
    }

    pub fn decl_at_pos(&self, pos: Position) -> Option<Element> {
        let matching = |range: lsp_types::Range| range.start <= pos && pos < range.end;

        self.headings
            .iter()
            .find(|n| matching(n.range))
            .map(|n| Element::H(n.clone()))
            .or_else(|| {
                self.link_defs
                    .iter()
                    .find(|n| matching(n.range))
                    .map(|n| Element::MLD(n.clone()))
            })
    }
}

/// Convenience re-export so callers can name CST node types through `index`.
pub type CstElement = cst::Element;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ParserSettings;
    use crate::parser::parse;
    use crate::structure::Structure;
    use crate::text::mk_text;

    fn index_of(content: &str) -> Index {
        let text = mk_text(content);
        let structure = parse(&ParserSettings::default(), &text);
        Index::of_cst(Structure::concrete_elements(&structure))
    }

    #[test]
    fn collects_elements_by_kind() {
        let index = index_of("# T\n\n## A\n\n[[w]] [m](u.md) #tag\n\n[lab]: /u\n");
        assert_eq!(index.titles.len(), 1);
        assert_eq!(index.headings.len(), 2);
        assert_eq!(index.wiki_links.len(), 1);
        assert_eq!(index.md_links.len(), 1);
        assert_eq!(index.tags.len(), 1);
        assert_eq!(index.link_defs.len(), 1);
    }

    #[test]
    fn groups_headings_by_slug() {
        let index = index_of("# A\n# A\n# B\n");
        assert_eq!(index.filter_heading_by_slug(&Slug::of_string("a")).len(), 1);
        // Disambiguated headings land under their own slug.
        assert_eq!(index.filter_heading_by_slug(&Slug::of_string("a-1")).len(), 1);
        assert_eq!(index.filter_heading_by_slug(&Slug::of_string("b")).len(), 1);
    }

    #[test]
    fn finds_link_def_by_normalized_label() {
        let index = index_of("[My  Label]: /u\n");
        assert!(index.try_find_link_def(&LinkLabel::of_string("my label")).is_some());
        assert!(index.try_find_link_def(&LinkLabel::of_string("other")).is_none());
    }

    #[test]
    fn locates_link_at_position() {
        let index = index_of("[[note]] text\n");
        let pos = Position { line: 0, character: 3 };
        assert!(matches!(index.link_at_pos(pos), Some(Element::WL(_))));
        assert!(index.link_at_pos(Position { line: 0, character: 8 }).is_none());
    }

    #[test]
    fn locates_declaration_at_position() {
        let index = index_of("# Title\n");
        assert!(matches!(
            index.decl_at_pos(Position { line: 0, character: 3 }),
            Some(Element::H(_))
        ));
    }
}
