//! The symbol model: definitions, references and tags, each optionally scoped to a document.
//!
//! Port of `Marksman.Syms`.

use crate::misc::{LinkLabel, Slug};
use crate::names::DocId;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IntraRef {
    IntraSection(Slug),
    IntraLinkDef(LinkLabel),
}

impl std::fmt::Display for IntraRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IntraRef::IntraSection(section) => write!(f, "[[#{section}]]"),
            IntraRef::IntraLinkDef(link_label) => write!(f, "[{link_label}]"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CrossRef {
    CrossDoc(String),
    CrossSection(String, Slug),
}

impl CrossRef {
    pub fn doc(&self) -> &str {
        match self {
            CrossRef::CrossDoc(doc) | CrossRef::CrossSection(doc, _) => doc,
        }
    }
}

impl std::fmt::Display for CrossRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CrossRef::CrossDoc(doc) => write!(f, "[[{doc}]]"),
            CrossRef::CrossSection(doc, section) => write!(f, "[[{doc}#{section}]]"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ref {
    IntraRef(IntraRef),
    CrossRef(CrossRef),
}

impl Ref {
    pub fn is_intra(&self) -> bool {
        matches!(self, Ref::IntraRef(_))
    }

    pub fn is_cross(&self) -> bool {
        matches!(self, Ref::CrossRef(_))
    }

    pub fn try_section(&self) -> Option<&Slug> {
        match self {
            Ref::CrossRef(CrossRef::CrossSection(_, section))
            | Ref::IntraRef(IntraRef::IntraSection(section)) => Some(section),
            _ => None,
        }
    }
}

impl std::fmt::Display for Ref {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Ref::IntraRef(intra) => write!(f, "{intra}"),
            Ref::CrossRef(cross) => write!(f, "{cross}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tag(pub String);

impl Tag {
    pub fn name(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Tag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Def {
    Doc,
    Title(String),
    Header(i32, String),
    LinkDef(LinkLabel),
}

impl Def {
    pub fn is_doc(&self) -> bool {
        matches!(self, Def::Doc)
    }

    pub fn is_title(&self) -> bool {
        matches!(self, Def::Title(_))
    }

    pub fn is_header_or_title(&self) -> bool {
        matches!(self, Def::Title(_) | Def::Header(..))
    }

    pub fn as_header(&self) -> Option<(i32, &str)> {
        match self {
            Def::Title(id) => Some((1, id)),
            Def::Header(level, id) => Some((*level, id)),
            _ => None,
        }
    }
}

impl std::fmt::Display for Def {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Def::Doc => f.write_str("Doc"),
            Def::Title(t) => write!(f, "T {{{t}}}"),
            Def::Header(l, h) => write!(f, "H{l} {{{h}}}"),
            Def::LinkDef(ld) => write!(f, "[{ld}]:"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sym {
    Def(Def),
    Ref(Ref),
    Tag(Tag),
}

impl Sym {
    pub fn as_ref(&self) -> Option<&Ref> {
        match self {
            Sym::Ref(r) => Some(r),
            _ => None,
        }
    }

    pub fn as_ref_cloned(&self) -> Option<Ref> {
        match self {
            Sym::Ref(r) => Some(r.clone()),
            _ => None,
        }
    }

    pub fn as_def(&self) -> Option<&Def> {
        match self {
            Sym::Def(d) => Some(d),
            _ => None,
        }
    }

    pub fn as_tag(&self) -> Option<&Tag> {
        match self {
            Sym::Tag(t) => Some(t),
            _ => None,
        }
    }

    pub fn is_doc(&self) -> bool {
        self.as_def().is_some_and(Def::is_doc)
    }

    pub fn is_title(&self) -> bool {
        self.as_def().is_some_and(Def::is_title)
    }

    pub fn is_ref_with_explicit_doc(&self) -> bool {
        self.as_ref().is_some_and(Ref::is_cross)
    }

    pub fn scoped(&self, scope: Scope) -> ScopedSym {
        (scope, self.clone())
    }

    pub fn scoped_to_doc(&self, doc_id: DocId) -> ScopedSym {
        (Scope::Doc(doc_id), self.clone())
    }
}

impl std::fmt::Display for Sym {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Sym::Ref(r) => write!(f, "{r}"),
            Sym::Def(d) => write!(f, "{d}"),
            Sym::Tag(t) => write!(f, "{t}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Scope {
    Doc(DocId),
    Global,
}

impl Scope {
    pub fn as_doc(&self) -> Option<&DocId> {
        match self {
            Scope::Doc(id) => Some(id),
            Scope::Global => None,
        }
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Scope::Doc(doc_id) => write!(f, "{doc_id}"),
            Scope::Global => f.write_str("Global"),
        }
    }
}

pub type ScopedSym = (Scope, Sym);

pub fn as_scoped_ref(scoped: &ScopedSym) -> Option<(Scope, Ref)> {
    match scoped {
        (scope, Sym::Ref(r)) => Some((scope.clone(), r.clone())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_render_like_the_original() {
        assert_eq!(Def::Doc.to_string(), "Doc");
        assert_eq!(Def::Title("t".into()).to_string(), "T {t}");
        assert_eq!(Def::Header(2, "h".into()).to_string(), "H2 {h}");
        assert_eq!(
            Def::LinkDef(LinkLabel::of_string("Lab")).to_string(),
            "[lab]:"
        );
        assert_eq!(Tag("x".into()).to_string(), "#x");
        assert_eq!(
            Ref::CrossRef(CrossRef::CrossSection("d".into(), Slug::of_string("S"))).to_string(),
            "[[d#s]]"
        );
        assert_eq!(
            Ref::IntraRef(IntraRef::IntraSection(Slug::of_string("S"))).to_string(),
            "[[#s]]"
        );
    }

    #[test]
    fn header_projection_maps_title_to_level_one() {
        assert_eq!(Def::Title("t".into()).as_header(), Some((1, "t")));
        assert_eq!(Def::Header(3, "h".into()).as_header(), Some((3, "h")));
        assert_eq!(Def::Doc.as_header(), None);
    }

    #[test]
    fn section_is_shared_between_intra_and_cross_refs() {
        let slug = Slug::of_string("Heading");
        assert_eq!(
            Ref::IntraRef(IntraRef::IntraSection(slug.clone())).try_section(),
            Some(&slug)
        );
        assert_eq!(
            Ref::CrossRef(CrossRef::CrossSection("d".into(), slug.clone())).try_section(),
            Some(&slug)
        );
        assert_eq!(Ref::CrossRef(CrossRef::CrossDoc("d".into())).try_section(), None);
    }
}
