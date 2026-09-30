//! Shared fixtures for the ported test suites.
//!
//! Mirrors `Tests/Helpers.fs` from the original so that ported cases can keep
//! their inputs and expectations verbatim.

#![allow(dead_code)]

use marksman::code_actions::DocumentAction;
use marksman::config::{Config, ParserSettings};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::misc;
use marksman::names::{DocId, FolderId};
use marksman::paths::{AbsPath, LocalPath, UriWith};
use marksman::text::{mk_text, Text};

pub fn path_to_uri(path: &str) -> String {
    format!("file://{path}")
}

pub fn dummy_root() -> &'static str {
    "/"
}

pub fn dummy_root_uri() -> String {
    path_to_uri(dummy_root())
}

pub fn dummy_root_path(path_comps: &[&str]) -> String {
    format!("{}{}", dummy_root(), path_comps.join("/"))
}

pub fn path_comps(path: &str) -> Vec<String> {
    path.trim_start_matches('/')
        .split('/')
        .map(|s| s.to_string())
        .collect()
}

pub fn mk_folder_id(str_: &str) -> FolderId {
    let root = AbsPath::of_system(str_);
    let uri = root.to_uri();
    UriWith::mk_root(uri)
}

pub fn mk_doc_id(folder_id: &FolderId, str_: &str) -> DocId {
    let path = LocalPath::of_system(str_);
    DocId::mk_rooted(folder_id, path)
}

/// Applies a code action's edit to the document text, as an editor would.
pub fn apply_document_action(doc: &Doc, action: &DocumentAction) -> String {
    let (before, after) = doc.text().cutout(action.edit);
    format!("{before}{}{after}", action.new_text)
}

/// Removes a leading `|` margin from every line, like F#'s triple-quoted
/// multi-line strings used in the original tests.
pub fn strip_margin(str_: &str) -> String {
    let lines: Vec<String> = misc::lines_of(str_)
        .into_iter()
        .map(|line| {
            let trimmed_start = line.trim_start();
            if let Some(rest) = trimmed_start.strip_prefix('|') {
                rest.to_string()
            } else {
                line.to_string()
            }
        })
        .collect();
    lines.join("\n")
}

pub fn strip_margin_trim(str_: &str) -> String {
    strip_margin(str_.trim())
}

/// Stands in for `Helpers.FakeDoc`.
pub struct FakeDoc;

impl FakeDoc {
    pub fn mk(content: &str) -> Doc {
        FakeDoc::mk_with(content, "fake.md", None, None)
    }

    pub fn mk_lines(lines: &[&str]) -> Doc {
        FakeDoc::mk(&lines.join("\n"))
    }

    pub fn mk_at(content: &str, path: &str) -> Doc {
        FakeDoc::mk_with(content, path, None, None)
    }

    pub fn mk_config(content: &str, config: &Config) -> Doc {
        FakeDoc::mk_with(content, "fake.md", None, Some(config.clone()))
    }

    pub fn mk_with(
        content: &str,
        path: &str,
        root: Option<&[&str]>,
        config: Option<Config>,
    ) -> Doc {
        let text = mk_text(content);
        let path_uri = path_to_uri(&dummy_root_path(
            &path_comps(path).iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        ));
        let root_comps: Vec<&str> = root.map(|r| r.to_vec()).unwrap_or_default();
        let root_uri = path_to_uri(&dummy_root_path(&root_comps));
        let config = config.unwrap_or_default();

        let doc_id = DocId::mk_rooted(&UriWith::mk_root(root_uri), LocalPath::of_uri(&path_uri));

        Doc::mk(&ParserSettings::of_config(&config), doc_id, None, text)
            .expect("fake document parses")
    }
}

/// Stands in for `Helpers.FakeFolder`.
pub struct FakeFolder;

impl FakeFolder {
    pub fn mk(docs: Vec<Doc>) -> Folder {
        FakeFolder::mk_with_config(docs, None)
    }

    pub fn mk_with_config(docs: Vec<Doc>, config: Option<Config>) -> Folder {
        let folder_id = UriWith::mk_root(dummy_root_uri());
        Folder::multi_file("dummy".to_string(), folder_id, docs, config)
    }
}

/// A temporary on-disk workspace, for tests that exercise folder loading.
pub struct TempWorkspace {
    pub path: std::path::PathBuf,
}

impl TempWorkspace {
    pub fn new(name: &str) -> TempWorkspace {
        let path = std::env::temp_dir().join(format!("marksman-tests-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).expect("create temp workspace");
        TempWorkspace { path }
    }

    /// Adds a project marker so the folder counts as a real workspace root.
    pub fn with_marker(self) -> TempWorkspace {
        std::fs::write(self.path.join(".marksman.toml"), b"").expect("write marker");
        self
    }

    pub fn write(&self, rel: &str, content: &str) {
        let path = self.path.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create parent dir");
        }
        std::fs::write(path, content).expect("write fixture");
    }

    pub fn folder_id(&self) -> FolderId {
        FolderId::of_uri(marksman::paths::system_path_to_uri_string(
            self.path.to_str().expect("utf-8 temp path"),
        ))
    }

    pub fn load(&self) -> Folder {
        Folder::try_load(None, "test", self.folder_id()).expect("folder loads")
    }

    pub fn load_with_config(&self, config: Config) -> Folder {
        Folder::try_load(Some(config), "test", self.folder_id()).expect("folder loads")
    }

    pub fn doc_named(&self, folder: &Folder, name: &str) -> Doc {
        folder
            .docs()
            .into_iter()
            .find(|d| d.name() == name)
            .unwrap_or_else(|| panic!("no document named {name:?}"))
    }
}

impl Drop for TempWorkspace {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.path).ok();
    }
}

/// Renders concrete elements the way the original's inline snapshots do.
pub fn fmt_elements(elements: &[marksman::cst::Element]) -> Vec<String> {
    elements
        .iter()
        .flat_map(|el| {
            let text = el.fmt();
            misc::lines_of(&text)
                .into_iter()
                .map(|l| l.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

pub fn text_of(doc: &Doc) -> &Text {
    doc.text()
}
