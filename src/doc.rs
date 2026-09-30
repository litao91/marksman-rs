//! A document: identity, text, parsed structure and index.
//!
//! Port of `Marksman.Doc`. The parsed payload is shared behind an `Arc` so that
//! documents can be cloned freely while still supporting the identity check the
//! folder difference uses to detect reopens.

use std::sync::Arc;

use lsp_types::{TextDocumentItem, Uri};

use crate::config::ParserSettings;
use crate::cst::{Cst, Heading, Node};
use crate::index::Index;
use crate::misc::Slug;
use crate::names::{DocId, FolderId};
use crate::parser::parse;
use crate::paths::{AbsPath, LocalPath, RelPath, RootPath, RootedRelPath};
use crate::structure::Structure;
use crate::syms::Sym;
use crate::text::{apply_text_change, mk_text, Text};

#[derive(Debug)]
struct Parsed {
    structure: Structure,
    index: Index,
}

#[derive(Debug)]
pub struct DocError {
    pub doc: RootedRelPath,
    pub cause: String,
}

impl std::fmt::Display for DocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Error while processing {}\n{}", self.doc.filename(), self.cause)
    }
}

#[derive(Clone, Debug)]
pub struct Doc {
    id: DocId,
    version: Option<i32>,
    text: Arc<Text>,
    parsed: Arc<Parsed>,
}

impl PartialEq for Doc {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.text.content == other.text.content
    }
}
impl Eq for Doc {}

impl PartialOrd for Doc {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Doc {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.id
            .cmp(&other.id)
            .then_with(|| self.text.content.cmp(&other.text.content))
    }
}

impl std::hash::Hash for Doc {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.text.content.hash(state);
    }
}

impl Doc {
    pub fn mk(
        parser_settings: &ParserSettings,
        id: DocId,
        version: Option<i32>,
        text: Text,
    ) -> Result<Doc, DocError> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let structure = parse(parser_settings, &text);
            let index = Index::of_cst(Structure::concrete_elements(&structure));
            Parsed { structure, index }
        }));

        match result {
            Ok(parsed) => Ok(Doc {
                id,
                version,
                text: Arc::new(text),
                parsed: Arc::new(parsed),
            }),
            Err(_) => Err(DocError {
                doc: id.path().clone(),
                cause: "failed to parse document".to_string(),
            }),
        }
    }

    pub fn id(&self) -> &DocId {
        &self.id
    }

    pub fn text(&self) -> &Text {
        &self.text
    }

    pub fn root_path(&self) -> &RootPath {
        self.id.path().root_path()
    }

    pub fn rel_path(&self) -> RelPath {
        self.id.rel_path_forced()
    }

    pub fn with_text(&self, config: &ParserSettings, new_text: Text) -> Doc {
        let structure = parse(config, &new_text);
        let index = Index::of_cst(Structure::concrete_elements(&structure));
        Doc {
            text: Arc::new(new_text),
            parsed: Arc::new(Parsed { structure, index }),
            ..self.clone()
        }
    }

    /// Whether this document shares its parsed payload with `other`, which is how
    /// a reopen with unchanged content is distinguished from a real edit.
    pub fn shares_parsed_with(&self, other: &Doc) -> bool {
        Arc::ptr_eq(&self.parsed, &other.parsed)
    }

    pub fn apply_lsp_change(
        parser_settings: &ParserSettings,
        changes: &[lsp_types::TextDocumentContentChangeEvent],
        version: i32,
        doc: &Doc,
    ) -> Result<Doc, DocError> {
        log::trace!(
            "Processing text change: uri={}, currentVersion={:?}, newVersion={}",
            doc.id.uri(),
            doc.version,
            version
        );
        let new_text = apply_text_change(changes, (*doc.text).clone());
        let mut updated = doc.with_text(parser_settings, new_text);
        updated.version = Some(version);
        Ok(updated)
    }

    pub fn from_lsp(
        parser_settings: &ParserSettings,
        folder_id: &FolderId,
        item: &TextDocumentItem,
    ) -> Result<Doc, DocError> {
        let path = LocalPath::of_uri(item.uri.as_str());
        let id = DocId::mk_rooted(folder_id, path);
        let text = mk_text(item.text.clone());
        Doc::mk(parser_settings, id, Some(item.version), text)
    }

    pub fn try_load(
        parser_settings: &ParserSettings,
        folder_id: &FolderId,
        path: &LocalPath,
    ) -> Option<Doc> {
        let content = std::fs::read_to_string(match path {
            LocalPath::Abs(p) => p.to_system().to_string(),
            LocalPath::Rel(p) => p.to_system().to_string(),
        })
        .ok()?;
        let text = mk_text(content);
        let id = DocId::mk_rooted(folder_id, path.clone());
        Doc::mk(parser_settings, id, None, text).ok()
    }

    pub fn uri(&self) -> &str {
        self.id.uri()
    }

    pub fn path(&self) -> AbsPath {
        self.id.path().to_abs()
    }

    pub fn path_from_root(&self) -> RelPath {
        self.id.rel_path_forced()
    }

    pub fn title(&self) -> Option<&Node<Heading>> {
        self.parsed.index.title()
    }

    pub fn index(&self) -> &Index {
        &self.parsed.index
    }

    pub fn structure(&self) -> &Structure {
        &self.parsed.structure
    }

    pub fn cst(&self) -> &Cst {
        self.parsed.structure.cst()
    }

    pub fn name(&self) -> String {
        match self.title() {
            Some(h) => h.data.name().to_string(),
            None => self.path_from_root().filename_stem().to_string(),
        }
    }

    pub fn slug(&self) -> Slug {
        Slug::of_string(&self.name())
    }

    pub fn version(&self) -> Option<i32> {
        self.version
    }

    pub fn set_version(&mut self, version: Option<i32>) {
        self.version = version;
    }

    pub fn syms(&self) -> &std::collections::BTreeSet<Sym> {
        self.parsed.structure.symbols()
    }

    pub fn rooted_rel_path(&self) -> &RootedRelPath {
        self.id.path()
    }
}

/// Builds a document directly from a URI, used when the enclosing folder is
/// unknown.
pub fn doc_id_from_uri(folder_id: &FolderId, uri: &Uri) -> DocId {
    DocId::mk_rooted(folder_id, LocalPath::of_uri(uri.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn folder() -> FolderId {
        FolderId::of_uri("file:///w/space")
    }

    fn doc(content: &str, rel: &str) -> Doc {
        let settings = ParserSettings::of_config(&Config::default_config());
        let id = DocId::mk_rooted(&folder(), LocalPath::of_system(&format!("/w/space/{rel}")));
        Doc::mk(&settings, id, Some(1), mk_text(content)).unwrap()
    }

    #[test]
    fn name_prefers_document_title() {
        assert_eq!(doc("# My Note\n\ntext\n", "file.md").name(), "My Note");
    }

    #[test]
    fn name_falls_back_to_file_stem() {
        assert_eq!(doc("no heading here\n", "some-file.md").name(), "some-file");
    }

    #[test]
    fn slug_of_name_is_the_addressable_key() {
        assert_eq!(doc("# My Note\n", "file.md").slug().to_string(), "my-note");
    }

    #[test]
    fn path_and_identity_come_from_the_id() {
        let d = doc("# T\n", "sub/file.md");
        assert_eq!(d.path().to_system(), "/w/space/sub/file.md");
        assert_eq!(d.path_from_root().to_system(), "sub/file.md");
        assert_eq!(d.uri(), "file:///w/space/sub/file.md");
        assert_eq!(d.root_path().to_system(), "/w/space");
    }

    #[test]
    fn text_change_reparses_and_bumps_version() {
        let d = doc("# Old\n", "file.md");
        let change = lsp_types::TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: "# New\n".to_string(),
        };
        let updated =
            Doc::apply_lsp_change(&ParserSettings::default(), &[change], 7, &d).unwrap();
        assert_eq!(updated.name(), "New");
        assert_eq!(updated.version(), Some(7));
    }

    #[test]
    fn equality_ignores_version() {
        let a = doc("# T\n", "file.md");
        let mut b = a.clone();
        b.set_version(Some(99));
        assert_eq!(a, b);
        assert!(a.shares_parsed_with(&b));
    }

    #[test]
    fn try_load_reads_from_disk() {
        let dir = std::env::temp_dir().join("marksman-doc-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("loaded.md");
        std::fs::write(&path, "# Loaded\n").unwrap();

        let folder_id = FolderId::of_uri(crate::paths::system_path_to_uri_string(
            dir.to_str().unwrap(),
        ));
        let loaded = Doc::try_load(
            &ParserSettings::default(),
            &folder_id,
            &LocalPath::of_system(path.to_str().unwrap()),
        );
        assert_eq!(loaded.map(|d| d.name()), Some("Loaded".to_string()));
        std::fs::remove_dir_all(&dir).ok();
    }
}
