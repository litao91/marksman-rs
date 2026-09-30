//! Identifiers and the names by which references address documents.
//!
//! Port of `Marksman.Names`.

use std::collections::BTreeSet;

use crate::misc::{self, Slug};
use crate::paths::{
    AbsPath, CanonDocPath, LocalPath, RelPath, RootPath, RootedRelPath, UriWith,
};

/// A percent-encoded URL as written in a Markdown link destination.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UrlEncoded(String);

impl UrlEncoded {
    pub fn mk_unchecked(s: impl Into<String>) -> UrlEncoded {
        UrlEncoded(s.into())
    }

    pub fn encode(str_: &str) -> UrlEncoded {
        UrlEncoded(misc::url_encode(str_))
    }

    pub fn decode(&self) -> String {
        misc::url_decode(&self.0)
    }

    pub fn raw(&self) -> &str {
        &self.0
    }
}

/// A wiki-link destination, escaped so that `#`, `|`, `[` and `]` cannot end the link.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WikiEncoded(String);

impl WikiEncoded {
    pub fn mk_unchecked(s: impl Into<String>) -> WikiEncoded {
        WikiEncoded(s.into())
    }

    pub fn encode(str_: &str) -> WikiEncoded {
        WikiEncoded(misc::encode_for_wiki(str_))
    }

    pub fn decode(&self) -> String {
        misc::url_decode(&self.0)
    }

    pub fn raw(&self) -> &str {
        &self.0
    }

    pub fn encode_as_string(str_: &str) -> String {
        WikiEncoded::encode(str_).0
    }
}

pub type FolderId = UriWith<RootPath>;

impl FolderId {
    pub fn of_uri(uri: impl Into<String>) -> FolderId {
        UriWith::mk_root(uri)
    }
}

/// Identifies a document by the URI of its path. Equality, ordering and hashing
/// all consider the URI alone.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DocId(pub UriWith<RootedRelPath>);

impl DocId {
    pub fn raw(&self) -> &UriWith<RootedRelPath> {
        &self.0
    }

    pub fn path(&self) -> &RootedRelPath {
        &self.0.data
    }

    pub fn rel_path_forced(&self) -> RelPath {
        self.0.data.rel_path_forced()
    }

    pub fn uri(&self) -> &str {
        &self.0.uri
    }

    pub fn root_path(&self) -> &RootPath {
        self.0.data.root_path()
    }

    pub fn mk_rooted(folder_id: &FolderId, path: LocalPath) -> DocId {
        DocId(UriWith::mk_rooted(folder_id, path))
    }
}

impl std::fmt::Display for DocId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0.data.rel_path_forced().to_system())
    }
}

/// A reference name together with the document it was written in. Path-shaped
/// names are relative to that source document.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InternName {
    pub src: DocId,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InternPath {
    ExactAbs(RootedRelPath),
    ExactRel(DocId, RootedRelPath),
    Approx(RelPath),
}

impl InternPath {
    pub fn to_rel(&self) -> RelPath {
        match self {
            InternPath::ExactAbs(rooted) | InternPath::ExactRel(_, rooted) => {
                rooted.rel_path_forced()
            }
            InternPath::Approx(path) => path.clone(),
        }
    }
}

impl InternName {
    pub fn mk_unchecked(src: DocId, name: impl Into<String>) -> InternName {
        InternName { src, name: name.into() }
    }

    pub fn mk_checked(exts: &[String], src: DocId, name: impl Into<String>) -> Option<InternName> {
        let name = name.into();
        if misc::is_potentially_internal_ref(exts, &name) {
            Some(InternName { src, name })
        } else {
            None
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn slug(&self) -> Slug {
        Slug::of_string(&self.name)
    }

    pub fn src(&self) -> &DocId {
        &self.src
    }

    pub fn try_as_path(&self) -> Option<InternPath> {
        if misc::is_well_formed_absolute_uri(&self.name) {
            return None;
        }

        if self.name.starts_with('/') {
            let decoded = misc::url_decode(self.name.trim_start_matches('/'));
            let rel_path = LocalPath::try_of_system(&decoded)?;
            let root_path = self.src.path().root_path().clone();
            let name_path = RootedRelPath::mk(root_path, rel_path);
            Some(InternPath::ExactAbs(name_path))
        } else {
            let decoded = misc::url_decode(&self.name);
            let raw_name_path = LocalPath::try_of_system(&decoded)?;

            if raw_name_path.has_dot_components() {
                let dir = self.src.path().directory()?;
                let combined = dir.combine(&raw_name_path)?;
                Some(InternPath::ExactRel(self.src.clone(), combined))
            } else {
                match raw_name_path {
                    LocalPath::Abs(_) => None,
                    LocalPath::Rel(path) => Some(InternPath::Approx(path)),
                }
            }
        }
    }

    pub fn as_path(&self) -> InternPath {
        self.try_as_path()
            .unwrap_or_else(|| panic!("Can't convert InternName {self:?} to a path"))
    }
}

/// Names by which references can address a document. A reference depends on
/// every alias through which a matching document could become visible.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DocumentAlias {
    TitleSlug(Slug),
    CanonicalPath(CanonDocPath),
    PathSuffix(Vec<String>),
}

impl DocumentAlias {
    pub fn of_reference_name(exts: &[String], name: &InternName) -> BTreeSet<DocumentAlias> {
        let title = DocumentAlias::TitleSlug(name.slug());

        match name.try_as_path() {
            None => BTreeSet::from([title]),
            Some(InternPath::ExactAbs(path) | InternPath::ExactRel(_, path)) => {
                let canon = CanonDocPath::mk(exts, &path.rel_path_forced());
                BTreeSet::from([title, DocumentAlias::CanonicalPath(canon)])
            }
            Some(InternPath::Approx(path)) => {
                let canon = CanonDocPath::mk(exts, &path);
                let parts = canon.components();
                if parts.is_empty() {
                    BTreeSet::from([title])
                } else {
                    BTreeSet::from([title, DocumentAlias::PathSuffix(parts)])
                }
            }
        }
    }

    pub fn of_document(
        exts: &[String],
        slug: &Slug,
        path: &RelPath,
    ) -> BTreeSet<DocumentAlias> {
        let canon = CanonDocPath::mk(exts, path);
        let components = canon.components();

        let mut aliases = BTreeSet::new();
        aliases.insert(DocumentAlias::TitleSlug(slug.clone()));
        aliases.insert(DocumentAlias::CanonicalPath(canon));

        for idx in 0..components.len() {
            aliases.insert(DocumentAlias::PathSuffix(components[idx..].to_vec()));
        }

        aliases
    }
}

/// Re-exported so callers can name the root type without importing `paths`.
pub type Root = AbsPath;
#[cfg(test)]
mod tests {
    use super::*;

    fn exts() -> Vec<String> {
        vec!["md".into(), "markdown".into()]
    }

    fn doc_id(root: &str, rel: &str) -> DocId {
        let folder = FolderId::of_uri(format!("file://{root}"));
        DocId::mk_rooted(&folder, LocalPath::of_system(&format!("{root}/{rel}")))
    }

    #[test]
    fn plain_name_is_approximate_path() {
        let name = InternName::mk_unchecked(doc_id("/w", "a.md"), "note");
        assert_eq!(name.try_as_path(), Some(InternPath::Approx(RelPath("note".into()))));
    }

    #[test]
    fn leading_slash_is_root_relative() {
        let name = InternName::mk_unchecked(doc_id("/w", "sub/a.md"), "/dir/note.md");
        match name.try_as_path() {
            Some(InternPath::ExactAbs(rooted)) => {
                assert_eq!(rooted.rel_path_forced().to_system(), "dir/note.md")
            }
            other => panic!("expected ExactAbs, got {other:?}"),
        }
    }

    #[test]
    fn dot_components_resolve_against_source_directory() {
        let name = InternName::mk_unchecked(doc_id("/w", "sub/a.md"), "../other/note.md");
        match name.try_as_path() {
            Some(InternPath::ExactRel(_, rooted)) => {
                assert_eq!(rooted.rel_path_forced().to_system(), "other/note.md")
            }
            other => panic!("expected ExactRel, got {other:?}"),
        }
    }

    #[test]
    fn external_uris_are_not_paths() {
        let name = InternName::mk_unchecked(doc_id("/w", "a.md"), "https://example.com/x");
        assert_eq!(name.try_as_path(), None);
    }

    #[test]
    fn percent_encoded_destinations_are_decoded() {
        let name = InternName::mk_unchecked(doc_id("/w", "a.md"), "dir/my%20note.md");
        assert_eq!(
            name.try_as_path(),
            Some(InternPath::Approx(RelPath("dir/my note.md".into())))
        );
    }

    #[test]
    fn document_aliases_cover_title_path_and_suffixes() {
        let aliases = DocumentAlias::of_document(
            &exts(),
            &Slug::of_string("My Note"),
            &RelPath("dir/sub/note.md".into()),
        );
        assert!(aliases.contains(&DocumentAlias::TitleSlug(Slug::of_string("My Note"))));
        assert!(aliases.contains(&DocumentAlias::CanonicalPath(CanonDocPath::mk(
            &exts(),
            &RelPath("dir/sub/note.md".into())
        ))));
        assert!(aliases.contains(&DocumentAlias::PathSuffix(vec![
            "dir".into(),
            "sub".into(),
            "note".into()
        ])));
        assert!(aliases.contains(&DocumentAlias::PathSuffix(vec!["note".into()])));
        assert_eq!(aliases.len(), 5);
    }

    #[test]
    fn reference_aliases_depend_on_name_shape() {
        // A bare title still yields a path-suffix alias: any document whose
        // path ends with that name is a candidate.
        let by_title = DocumentAlias::of_reference_name(
            &exts(),
            &InternName::mk_unchecked(doc_id("/w", "a.md"), "Some Note"),
        );
        assert_eq!(by_title.len(), 2);
        assert!(by_title.contains(&DocumentAlias::TitleSlug(Slug::of_string("Some Note"))));
        assert!(by_title.contains(&DocumentAlias::PathSuffix(vec!["Some Note".to_string()])));

        let by_path = DocumentAlias::of_reference_name(
            &exts(),
            &InternName::mk_unchecked(doc_id("/w", "a.md"), "dir/note.md"),
        );
        assert!(by_path.iter().any(|a| matches!(a, DocumentAlias::PathSuffix(p) if p == &vec!["dir".to_string(), "note".to_string()])));
    }
}
