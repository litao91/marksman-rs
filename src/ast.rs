//! The abstract element model: what a document *declares* and what it *refers to*.
//!
//! Port of `Marksman.Ast`.

use crate::config::ParserSettings;
use crate::misc::{self, LinkLabel, Slug};
use crate::names::UrlEncoded;
use crate::syms::{CrossRef, Def, IntraRef, Ref, Sym, Tag as SymTag};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Heading {
    pub level: i32,
    pub is_title: bool,
    pub text: String,
    pub id: Slug,
}

impl Heading {
    pub fn compact_format(&self) -> String {
        let prefix = "#".repeat(self.level.max(0) as usize);
        format!("{prefix} {} {{{}}}", self.text, self.id.to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WikiLink {
    pub doc: Option<String>,
    pub heading: Option<String>,
}

impl WikiLink {
    pub fn compact_format(&self) -> String {
        let doc = self.doc.clone().unwrap_or_default();
        let heading = self
            .heading
            .as_ref()
            .map(|x| format!("#{x}"))
            .unwrap_or_default();
        format!("[[{doc}{heading}]]")
    }
}

/// `[text](url "title")`
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MdLink {
    pub text: String,
    pub url: Option<String>,
    pub anchor: Option<String>,
}

impl MdLink {
    pub fn compact_format(&self) -> String {
        let url = self.url.clone().unwrap_or_default();
        let anchor = self.anchor.as_ref().map(|x| format!("#{x}")).unwrap_or_default();
        format!("[{}]({url}{anchor})", self.text)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MdRef {
    /// `[text][dest]`
    Full(String, String),
    /// `[dest][]`
    Collapsed(String),
    /// `[dest]`
    Shortcut(String),
}

impl MdRef {
    pub fn compact_format(&self) -> String {
        match self {
            MdRef::Full(text, dest) => format!("[{text}][{dest}]"),
            MdRef::Collapsed(dest) => format!("[{dest}][]"),
            MdRef::Shortcut(dest) => format!("[{dest}]"),
        }
    }

    pub fn dest(&self) -> &str {
        match self {
            MdRef::Full(_, dest) | MdRef::Collapsed(dest) | MdRef::Shortcut(dest) => dest,
        }
    }

    pub fn dest_label(&self) -> LinkLabel {
        LinkLabel::of_string(self.dest())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MdLinkDef {
    pub label: String,
    pub url: UrlEncoded,
}

impl MdLinkDef {
    pub fn label_normalized(&self) -> LinkLabel {
        LinkLabel::of_string(&self.label)
    }

    pub fn compact_format(&self) -> String {
        format!("[{}]: {}", self.label, self.url.raw())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Element {
    H(Heading),
    WL(WikiLink),
    ML(MdLink),
    MR(MdRef),
    MLD(MdLinkDef),
    T(String),
}

impl Element {
    pub fn compact_format(&self) -> String {
        match self {
            Element::H(heading) => heading.compact_format(),
            Element::WL(wiki_link) => wiki_link.compact_format(),
            Element::ML(md_link) => md_link.compact_format(),
            Element::MR(md_ref) => md_ref.compact_format(),
            Element::MLD(md_link_def) => md_link_def.compact_format(),
            Element::T(tag) => format!("#{tag}"),
        }
    }

    pub fn as_heading(&self) -> Option<&Heading> {
        match self {
            Element::H(h) => Some(h),
            _ => None,
        }
    }

    pub fn as_link_def(&self) -> Option<&MdLinkDef> {
        match self {
            Element::MLD(mld) => Some(mld),
            _ => None,
        }
    }

    /// Projects an element onto the symbol it contributes, if any. Elements that
    /// carry only whitespace produce no symbol.
    pub fn to_sym(&self, parser_settings: &ParserSettings) -> Option<Sym> {
        match self {
            Element::H(Heading { level, is_title, id, .. }) => {
                if id.is_empty() {
                    None
                } else if *is_title {
                    Some(Sym::Def(Def::Title(id.as_str().to_string())))
                } else {
                    Some(Sym::Def(Def::Header(*level, id.as_str().to_string())))
                }
            }
            Element::WL(WikiLink { doc: None, heading: None }) => None,
            Element::WL(WikiLink { doc: Some(doc), heading: None }) => {
                if doc.trim().is_empty() {
                    None
                } else {
                    Some(Sym::Ref(Ref::CrossRef(CrossRef::CrossDoc(doc.clone()))))
                }
            }
            Element::WL(WikiLink { doc: Some(doc), heading: Some(heading) }) => {
                if heading.trim().is_empty() {
                    None
                } else {
                    Some(Sym::Ref(Ref::CrossRef(CrossRef::CrossSection(
                        doc.clone(),
                        Slug::of_string(heading),
                    ))))
                }
            }
            Element::WL(WikiLink { doc: None, heading: Some(heading) }) => {
                if heading.trim().is_empty() {
                    None
                } else {
                    Some(Sym::Ref(Ref::IntraRef(IntraRef::IntraSection(Slug::of_string(
                        heading,
                    )))))
                }
            }
            Element::ML(MdLink { url, anchor, .. }) => {
                let exts = &parser_settings.md_file_ext;
                match (url, anchor) {
                    (None, None) => None,
                    (None, Some(anchor)) => {
                        if anchor.trim().is_empty() {
                            None
                        } else {
                            Some(Sym::Ref(Ref::IntraRef(IntraRef::IntraSection(
                                Slug::of_string(anchor),
                            ))))
                        }
                    }
                    (Some(url), _) if !misc::is_potentially_internal_ref(exts, url) => None,
                    (Some(url), None) => {
                        if url.trim().is_empty() {
                            None
                        } else {
                            Some(Sym::Ref(Ref::CrossRef(CrossRef::CrossDoc(url.clone()))))
                        }
                    }
                    (Some(url), Some(anchor)) => {
                        if url.trim().is_empty() || anchor.trim().is_empty() {
                            None
                        } else {
                            Some(Sym::Ref(Ref::CrossRef(CrossRef::CrossSection(
                                url.clone(),
                                Slug::of_string(anchor),
                            ))))
                        }
                    }
                }
            }
            Element::MR(md_ref) => Some(Sym::Ref(Ref::IntraRef(IntraRef::IntraLinkDef(
                md_ref.dest_label(),
            )))),
            Element::MLD(md_link_def) => Some(Sym::Def(Def::LinkDef(md_link_def.label_normalized()))),
            Element::T(tag) => Some(Sym::Tag(SymTag(tag.clone()))),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Ast {
    pub elements: Vec<Element>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ParserSettings {
        ParserSettings::default()
    }

    #[test]
    fn title_heading_becomes_title_definition() {
        let el = Element::H(Heading {
            level: 1,
            is_title: true,
            text: "My Note".into(),
            id: Slug::of_string("My Note"),
        });
        assert_eq!(
            el.to_sym(&settings()),
            Some(Sym::Def(Def::Title("my-note".into())))
        );
        assert_eq!(el.compact_format(), "# My Note {my-note}");
    }

    #[test]
    fn non_title_heading_keeps_its_level() {
        let el = Element::H(Heading {
            level: 3,
            is_title: false,
            text: "Sub".into(),
            id: Slug::of_string("Sub"),
        });
        assert_eq!(el.to_sym(&settings()), Some(Sym::Def(Def::Header(3, "sub".into()))));
        assert_eq!(el.compact_format(), "### Sub {sub}");
    }

    #[test]
    fn empty_slug_produces_no_symbol() {
        let el = Element::H(Heading {
            level: 2,
            is_title: false,
            text: "🚀".into(),
            id: Slug::of_string("🚀"),
        });
        assert_eq!(el.to_sym(&settings()), None);
    }

    #[test]
    fn wiki_link_shapes_map_to_different_refs() {
        let s = settings();
        assert_eq!(
            Element::WL(WikiLink { doc: Some("note".into()), heading: None }).to_sym(&s),
            Some(Sym::Ref(Ref::CrossRef(CrossRef::CrossDoc("note".into()))))
        );
        assert_eq!(
            Element::WL(WikiLink {
                doc: Some("note".into()),
                heading: Some("A B".into())
            })
            .to_sym(&s),
            Some(Sym::Ref(Ref::CrossRef(CrossRef::CrossSection("note".into(), Slug::of_string("A B")))))
        );
        assert_eq!(
            Element::WL(WikiLink { doc: None, heading: Some("A B".into()) }).to_sym(&s),
            Some(Sym::Ref(Ref::IntraRef(IntraRef::IntraSection(Slug::of_string("A B")))))
        );
        assert_eq!(Element::WL(WikiLink { doc: None, heading: None }).to_sym(&s), None);
        assert_eq!(
            Element::WL(WikiLink { doc: Some("  ".into()), heading: None }).to_sym(&s),
            None
        );
    }

    #[test]
    fn external_urls_produce_no_symbol() {
        let el = Element::ML(MdLink {
            text: "t".into(),
            url: Some("https://example.com".into()),
            anchor: None,
        });
        assert_eq!(el.to_sym(&settings()), None);
    }

    #[test]
    fn internal_markdown_link_becomes_cross_ref() {
        let el = Element::ML(MdLink {
            text: "t".into(),
            url: Some("other.md".into()),
            anchor: Some("Section".into()),
        });
        assert_eq!(
            el.to_sym(&settings()),
            Some(Sym::Ref(Ref::CrossRef(CrossRef::CrossSection(
                "other.md".into(),
                Slug::of_string("Section")
            ))))
        );
    }

    #[test]
    fn anchor_only_markdown_link_is_intra_ref() {
        let el = Element::ML(MdLink { text: "t".into(), url: None, anchor: Some("head".into()) });
        assert_eq!(
            el.to_sym(&settings()),
            Some(Sym::Ref(Ref::IntraRef(IntraRef::IntraSection(Slug::of_string("head")))))
        );
    }

    #[test]
    fn reference_links_and_definitions_use_normalized_labels() {
        let mr = Element::MR(MdRef::Full("text".into(), "My  Label".into()));
        assert_eq!(
            mr.to_sym(&settings()),
            Some(Sym::Ref(Ref::IntraRef(IntraRef::IntraLinkDef(LinkLabel::of_string(
                "my label"
            )))))
        );

        let mld = Element::MLD(MdLinkDef {
            label: "My Label".into(),
            url: UrlEncoded::mk_unchecked("/url"),
        });
        assert_eq!(
            mld.to_sym(&settings()),
            Some(Sym::Def(Def::LinkDef(LinkLabel::of_string("my label"))))
        );
        assert_eq!(mld.compact_format(), "[My Label]: /url");
    }

    #[test]
    fn tag_becomes_tag_symbol() {
        assert_eq!(Element::T("todo".into()).to_sym(&settings()), Some(Sym::Tag(SymTag("todo".into()))));
    }
}
