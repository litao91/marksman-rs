//! A workspace folder: its documents, the indexes over them, and the connection graph.
//!
//! Port of `Marksman.Folder`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use log::{error, info, trace, warn};

use crate::config::{Config, ParserSettings};
use crate::conn::{
    CandidateDocumentResolution, Conn, ConnectionChange, DefinitionSelector, DocumentChange,
    DocumentInput, Oracle, OracleApi,
};
use crate::doc::Doc;
use crate::gitignore::Matcher;
use crate::misc::{sorted_merge_iter, MergeEntry};
use crate::misc::{self, Slug};
use crate::mmap::MMap;
use crate::names::{DocumentAlias, DocId, FolderId, InternName, InternPath};
use crate::paths::{AbsPath, CanonDocPath, LocalPath, RelPath, RootPath, RootedRelPath};
use crate::suffix_tree::SuffixTree;
use crate::syms::{Def, Sym};

const IGNORE_FILES: [&str; 3] = [".ignore", ".gitignore", ".hgignore"];
const MARKER_FILES: [&str; 1] = [".marksman.toml"];
const MARKER_DIRS: [&str; 4] = [".git", ".hg", ".svn", ".jj"];

#[derive(Clone, Debug)]
pub struct MultiFile {
    pub name: String,
    pub root: FolderId,
    pub docs: BTreeMap<RelPath, Doc>,
    pub config: Option<Config>,
}

impl MultiFile {
    pub fn root_path(&self) -> &RootPath {
        &self.root.data
    }
}

#[derive(Clone, Debug)]
pub struct SingleFile {
    pub doc: Doc,
    pub config: Option<Config>,
}

#[derive(Clone, Debug)]
pub enum FolderData {
    MultiFile(Arc<MultiFile>),
    SingleFile(Arc<SingleFile>),
}

impl FolderData {
    pub fn config(&self) -> Option<Config> {
        match self {
            FolderData::SingleFile(s) => s.config.clone(),
            FolderData::MultiFile(m) => m.config.clone(),
        }
    }

    pub fn config_or_default(&self) -> Config {
        Config::or_default(self.config())
    }

    pub fn docs(&self) -> Vec<Doc> {
        match self {
            FolderData::SingleFile(s) => vec![s.doc.clone()],
            FolderData::MultiFile(m) => m.docs.values().cloned().collect(),
        }
    }

    pub fn try_find_doc_by_rel_path(&self, path: &RelPath) -> Option<Doc> {
        match self {
            FolderData::SingleFile(s) => {
                let stem = s.doc.path().filename_stem().to_string();
                if stem.ends_with(path.filename_stem()) {
                    Some(s.doc.clone())
                } else {
                    None
                }
            }
            FolderData::MultiFile(m) => m.docs.get(path).cloned(),
        }
    }

    pub fn try_find_doc_by_path(&self, uri: &AbsPath) -> Option<Doc> {
        match self {
            FolderData::SingleFile(s) => {
                if &s.doc.path() == uri {
                    Some(s.doc.clone())
                } else {
                    None
                }
            }
            FolderData::MultiFile(m) => {
                let rooted = RootedRelPath::mk(m.root.data.clone(), LocalPath::Abs(uri.clone()));
                m.docs.get(&rooted.rel_path_forced()).cloned()
            }
        }
    }

    pub fn try_find_doc_by_id(&self, id: &DocId) -> Option<Doc> {
        let rel = id.path().rel_path_forced();
        self.try_find_doc_by_rel_path(&rel)
    }

    pub fn find_doc_by_id(&self, id: &DocId) -> Doc {
        self.try_find_doc_by_id(id)
            .unwrap_or_else(|| panic!("Expected doc could not be found: {}", id.uri()))
    }

    pub fn syms(&self) -> MMap<DocId, Sym> {
        let mut mapping = MMap::empty();
        for doc in self.docs() {
            for sym in doc.syms() {
                mapping.add_mut(doc.id().clone(), sym.clone());
            }
        }
        mapping
    }
}

#[derive(Clone, Debug)]
pub struct FolderLookup {
    pub docs_by_slug: BTreeMap<Slug, BTreeSet<Doc>>,
    pub docs_by_path: SuffixTree<CanonDocPath, Doc>,
    pub config: Option<Config>,
}

impl FolderLookup {
    pub fn of_data(data: &FolderData) -> FolderLookup {
        let config = data.config();
        let md_ext = data.config_or_default().core_markdown_file_extensions();

        match data {
            FolderData::SingleFile(s) => {
                let mut by_slug = BTreeMap::new();
                by_slug.insert(s.doc.slug(), BTreeSet::from([s.doc.clone()]));

                let path = CanonDocPath::mk(&md_ext, &s.doc.path_from_root());
                let by_path = SuffixTree::of_seq(
                    CanonDocPath::components,
                    std::iter::once((path, s.doc.clone())),
                );

                FolderLookup { docs_by_slug: by_slug, docs_by_path: by_path, config }
            }
            FolderData::MultiFile(m) => {
                let mut by_slug: BTreeMap<Slug, BTreeSet<Doc>> = BTreeMap::new();
                for doc in m.docs.values() {
                    by_slug.entry(doc.slug()).or_default().insert(doc.clone());
                }

                let entries = m.docs.values().map(|doc| {
                    (CanonDocPath::mk(&md_ext, &doc.path_from_root()), doc.clone())
                });
                let by_path = SuffixTree::of_seq(CanonDocPath::components, entries);

                FolderLookup { docs_by_slug: by_slug, docs_by_path: by_path, config }
            }
        }
    }

    fn canon_path(&self, doc: &Doc) -> CanonDocPath {
        let exts = Config::or_default(self.config.clone()).core_markdown_file_extensions();
        CanonDocPath::mk(&exts, &doc.path_from_root())
    }

    pub fn without_doc(&self, doc: &Doc) -> FolderLookup {
        let slug = doc.slug();
        let doc_path = self.canon_path(doc);

        let mut by_slug = self.docs_by_slug.clone();
        if let Some(docs) = by_slug.get_mut(&slug) {
            docs.remove(doc);
        }

        FolderLookup {
            docs_by_slug: by_slug,
            docs_by_path: self.docs_by_path.remove_value(&doc_path, doc.clone()),
            config: self.config.clone(),
        }
    }

    pub fn with_doc(&self, doc: &Doc) -> FolderLookup {
        let slug = doc.slug();
        let doc_path = self.canon_path(doc);

        let mut by_slug = self.docs_by_slug.clone();
        by_slug.entry(slug).or_default().insert(doc.clone());

        FolderLookup {
            docs_by_slug: by_slug,
            docs_by_path: self.docs_by_path.add(&doc_path, doc.clone()),
            config: self.config.clone(),
        }
    }

    /// Documents whose title slug matches.
    pub fn filter_docs_by_slug(&self, slug: &Slug) -> Vec<Doc> {
        self.docs_by_slug
            .get(slug)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }
}

/// Answers the two questions the connection graph asks of a folder snapshot.
pub struct FolderOracle {
    data: FolderData,
    lookup: Arc<FolderLookup>,
}

impl FolderOracle {
    pub fn resolve_documents_by_path(&self, path: &InternPath) -> Vec<Doc> {
        let exts = self.data.config_or_default().core_markdown_file_extensions();
        match path {
            InternPath::ExactAbs(rooted) | InternPath::ExactRel(_, rooted) => {
                let rel = rooted.rel_path_forced();
                match self.data.try_find_doc_by_rel_path(&rel) {
                    Some(doc) if doc.path_from_root() == rel => vec![doc],
                    _ => {
                        let canon = CanonDocPath::mk(&exts, &rel);
                        self.lookup.docs_by_path.find_exact_values(&canon).into_iter().collect()
                    }
                }
            }
            InternPath::Approx(rel_path) => {
                let canon = CanonDocPath::mk(&exts, rel_path);
                if canon.components().is_empty() {
                    Vec::new()
                } else {
                    self.lookup.docs_by_path.filter_matching_values(&canon)
                }
            }
        }
    }

    fn select_definitions(&self, selector: &DefinitionSelector) -> Vec<Def> {
        let scope = selector.scope();
        let Some(doc_id) = scope.as_doc() else { return Vec::new() };

        let definitions: Vec<Def> = self
            .data
            .find_doc_by_id(doc_id)
            .structure()
            .symbols()
            .iter()
            .filter_map(Sym::as_def)
            .filter(|d| selector.reads_definition(d))
            .cloned()
            .collect();

        match selector {
            DefinitionSelector::DocumentTarget(_) => {
                let titles: Vec<Def> = definitions.iter().filter(|d| d.is_title()).cloned().collect();
                if titles.is_empty() {
                    vec![Def::Doc]
                } else {
                    titles
                }
            }
            DefinitionSelector::SectionTarget(..)
            | DefinitionSelector::LinkDefinitionTarget(..) => definitions,
        }
    }
}

impl OracleApi for FolderOracle {
    fn resolve_candidate_documents(&self, name: &InternName) -> CandidateDocumentResolution {
        let exts = self.data.config_or_default().core_markdown_file_extensions();
        let aliases_read = DocumentAlias::of_reference_name(&exts, name);

        let mut documents = BTreeSet::new();
        for alias in &aliases_read {
            match alias {
                DocumentAlias::TitleSlug(slug) => {
                    for doc in self.lookup.filter_docs_by_slug(slug) {
                        documents.insert(doc.id().clone());
                    }
                }
                DocumentAlias::CanonicalPath(path) => {
                    for doc in self.lookup.docs_by_path.find_exact_values(path) {
                        documents.insert(doc.id().clone());
                    }
                }
                DocumentAlias::PathSuffix(parts) => {
                    let mut parts_rev = parts.clone();
                    parts_rev.reverse();
                    for doc in self.lookup.docs_by_path.filter_matching_parts(&parts_rev) {
                        documents.insert(doc.id().clone());
                    }
                }
            }
        }

        CandidateDocumentResolution { documents, aliases_read }
    }

    fn select_definitions(&self, selector: &DefinitionSelector) -> Vec<Def> {
        FolderOracle::select_definitions(self, selector)
    }
}

/// A folder snapshot. `data`, `lookup` and `conn` are all shared so that a
/// snapshot can be handed to a concurrent reader without copying the indexes.
#[derive(Clone, Debug)]
pub struct Folder {
    pub data: FolderData,
    pub lookup: Arc<FolderLookup>,
    pub conn: Arc<Conn>,
}

#[derive(Clone, Debug, Default)]
pub struct DocumentDifference {
    pub added: BTreeSet<DocId>,
    pub removed: BTreeSet<DocId>,
    pub changed: BTreeSet<DocId>,
    pub reopened: BTreeSet<DocId>,
}

impl DocumentDifference {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.removed.is_empty()
            && self.changed.is_empty()
            && self.reopened.is_empty()
    }
}

fn is_real_workspace_folder(root: &RootPath) -> bool {
    let root_path = Path::new(root.to_system());
    if !root_path.is_dir() {
        return false;
    }
    MARKER_FILES.iter().any(|m| root_path.join(m).is_file())
        || MARKER_DIRS.iter().any(|m| root_path.join(m).is_dir())
}

/// Some editors pass a folder that is not really a project root; indexing it
/// would pull in an unbounded subtree.
/// See https://github.com/helix-editor/helix/issues/4436 and
/// https://github.com/artempyanykh/marksman/discussions/377
pub fn check_workspace_folder_with_warn(folder_id: &FolderId) -> bool {
    if is_real_workspace_folder(&folder_id.data) {
        true
    } else {
        warn!("Workspace folder is bogus: root={}", folder_id.data.to_system());
        false
    }
}

fn read_ignore_files(root: &LocalPath) -> Vec<String> {
    let mut lines = Vec::new();
    for file in IGNORE_FILES {
        let path = root.append_file(file);
        let path_str = path.to_system().to_string();
        if Path::new(&path_str).is_file() {
            trace!("Reading ignore globs: file={path_str}");
            match std::fs::read_to_string(&path_str) {
                Ok(content) => lines.extend(misc::lines_of(&content).into_iter().map(|l| l.to_string())),
                Err(_) => trace!("Failed to read ignore globs: file={path_str}"),
            }
        }
    }
    lines
}

fn load_docs(parser_settings: &ParserSettings, folder_id: &FolderId) -> Vec<Doc> {
    fn collect(
        parser_settings: &ParserSettings,
        folder_id: &FolderId,
        cur: &LocalPath,
        ignore_matchers: &[Matcher],
        out: &mut Vec<Doc>,
    ) {
        let mut matchers: Vec<Matcher> = Vec::new();
        let pats = read_ignore_files(cur);
        if !pats.is_empty() {
            matchers.push(Matcher::mk(cur.to_system(), &pats));
        }
        matchers.extend_from_slice(ignore_matchers);

        let dir = Path::new(cur.to_system());
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                warn!("Couldn't read the folder: dir={}, error={err}", cur.to_system());
                return;
            }
        };

        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.is_file() {
                files.push(path);
            }
        }
        files.sort();
        dirs.sort();

        for file in files {
            let full = file.to_string_lossy().to_string();
            if misc::is_markdown_file(&parser_settings.md_file_ext, &full)
                && !Matcher::ignores_any(&matchers, &full)
            {
                if let Some(document) =
                    Doc::try_load(parser_settings, folder_id, &LocalPath::of_system(&full))
                {
                    out.push(document);
                }
            } else {
                trace!("Skipping ignored file: file={full}");
            }
        }

        for dir in dirs {
            let full = dir.to_string_lossy().to_string();
            if Matcher::ignores_any(&matchers, &full) {
                trace!("Skipping ignored directory: file={full}");
                continue;
            }
            collect(parser_settings, folder_id, &LocalPath::of_system(&full), &matchers, out);
        }
    }

    let mut out = Vec::new();
    collect(
        parser_settings,
        folder_id,
        &folder_id.data.to_local(),
        &[Matcher::mk_default(folder_id.data.to_system())],
        &mut out,
    );
    out
}

fn try_load_folder_config(folder_id: &FolderId) -> Option<Config> {
    let folder_config_path = folder_id.data.append_file(".marksman.toml");
    let path = folder_config_path.to_system().to_string();

    if Path::new(&path).is_file() {
        trace!("Found folder config: config={path}");
        let config = crate::config::read(&path);
        if config.is_none() {
            error!("Malformed folder config, skipping: config={path}");
        }
        config
    } else {
        trace!("No folder config found: path={path}");
        None
    }
}

fn document_input(doc: &Doc) -> DocumentInput {
    DocumentInput {
        id: doc.id().clone(),
        slug: doc.slug(),
        path: doc.path_from_root(),
        symbols: doc.syms().clone(),
    }
}

impl Folder {
    pub fn is_single_file(&self) -> bool {
        matches!(self.data, FolderData::SingleFile(_))
    }

    pub fn config(&self) -> Option<Config> {
        self.data.config()
    }

    pub fn config_or_default(&self) -> Config {
        self.data.config_or_default()
    }

    pub fn docs(&self) -> Vec<Doc> {
        self.data.docs()
    }

    pub fn conn(&self) -> &Conn {
        &self.conn
    }

    pub fn conn_arc(&self) -> &Arc<Conn> {
        &self.conn
    }

    pub fn id(&self) -> FolderId {
        match &self.data {
            FolderData::MultiFile(m) => m.root.clone(),
            FolderData::SingleFile(s) => crate::paths::UriWith {
                uri: s.doc.uri().to_string(),
                data: RootPath(s.doc.path()),
            },
        }
    }

    pub fn root_path(&self) -> RootPath {
        match &self.data {
            FolderData::MultiFile(m) => m.root.data.clone(),
            FolderData::SingleFile(s) => s.doc.root_path().clone(),
        }
    }

    pub fn try_find_doc_by_path(&self, uri: &AbsPath) -> Option<Doc> {
        self.data.try_find_doc_by_path(uri)
    }

    pub fn try_find_doc_by_rel_path(&self, path: &RelPath) -> Option<Doc> {
        match self.data.try_find_doc_by_rel_path(path) {
            Some(doc) => Some(doc),
            None => {
                let extensions = self.config_or_default().core_markdown_file_extensions();
                let canon_path = CanonDocPath::mk(&extensions, path);
                let matches: Vec<Doc> = self
                    .lookup
                    .docs_by_path
                    .find_exact_values(&canon_path)
                    .into_iter()
                    .collect();
                match matches.len() {
                    1 => Some(matches.into_iter().next().unwrap()),
                    _ => None,
                }
            }
        }
    }

    pub fn find_doc_by_id(&self, id: &DocId) -> Doc {
        self.data.find_doc_by_id(id)
    }

    /// Whether two folders are the very same snapshot. Diagnostics use this to
    /// skip comparison work entirely when nothing was rebuilt.
    pub fn same_snapshot(&self, other: &Folder) -> bool {
        match (&self.data, &other.data) {
            (FolderData::MultiFile(a), FolderData::MultiFile(b)) => Arc::ptr_eq(a, b),
            (FolderData::SingleFile(a), FolderData::SingleFile(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub fn oracle(&self) -> Oracle {
        Arc::new(FolderOracle { data: self.data.clone(), lookup: Arc::clone(&self.lookup) })
    }

    pub fn syms(&self) -> MMap<DocId, Sym> {
        self.data.syms()
    }

    /// Identifies added, removed, changed and reopened docs between two folders.
    pub fn docs_difference(before: &Folder, after: &Folder) -> DocumentDifference {
        fn keyed_docs(data: &FolderData) -> Vec<(RelPath, Doc)> {
            match data {
                FolderData::SingleFile(s) => vec![(s.doc.path_from_root(), s.doc.clone())],
                FolderData::MultiFile(m) => {
                    m.docs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                }
            }
        }

        let mut diff = DocumentDifference::default();

        sorted_merge_iter(
            keyed_docs(&before.data).into_iter(),
            keyed_docs(&after.data).into_iter(),
            |_, entry| match entry {
                MergeEntry::OnlyBefore(old_doc) => {
                    diff.removed.insert(old_doc.id().clone());
                }
                MergeEntry::OnlyAfter(new_doc) => {
                    diff.added.insert(new_doc.id().clone());
                }
                MergeEntry::Both(old_doc, new_doc) => {
                    if old_doc.id() != new_doc.id() {
                        diff.removed.insert(old_doc.id().clone());
                        diff.added.insert(new_doc.id().clone());
                    } else if !old_doc.shares_parsed_with(&new_doc) {
                        if old_doc != new_doc {
                            diff.changed.insert(old_doc.id().clone());
                        } else if old_doc.version().is_none() && new_doc.version().is_some() {
                            // Doc equality ignores version; reopening still needs publication.
                            diff.reopened.insert(old_doc.id().clone());
                        }
                    }
                }
            },
        );

        diff
    }

    fn mk(data: FolderData) -> Folder {
        let lookup = Arc::new(FolderLookup::of_data(&data));
        let oracle: Oracle = Arc::new(FolderOracle { data: data.clone(), lookup: Arc::clone(&lookup) });
        let conn = Arc::new(Conn::mk(&oracle, &data.syms()));
        Folder { data, lookup, conn }
    }

    pub fn single_file(doc: Doc, config: Option<Config>) -> Folder {
        Folder::mk(FolderData::SingleFile(Arc::new(SingleFile { doc, config })))
    }

    pub fn multi_file(name: String, root: FolderId, docs: Vec<Doc>, config: Option<Config>) -> Folder {
        let mut by_path = BTreeMap::new();
        for doc in docs {
            by_path.insert(doc.path_from_root(), doc);
        }
        Folder::mk(FolderData::MultiFile(Arc::new(MultiFile { name, root, docs: by_path, config })))
    }

    pub fn with_config(&self, config: Option<Config>) -> Folder {
        if config == self.data.config() {
            return self.clone();
        }
        match &self.data {
            FolderData::SingleFile(s) => Folder::mk(FolderData::SingleFile(Arc::new(SingleFile {
                doc: s.doc.clone(),
                config,
            }))),
            FolderData::MultiFile(m) => {
                // Extension changes also change canonical path keys. Rebuild the
                // document map, not just the indexes over its old keys.
                Folder::multi_file(
                    m.name.clone(),
                    m.root.clone(),
                    m.docs.values().cloned().collect(),
                    config,
                )
            }
        }
    }

    pub fn try_load(user_config: Option<Config>, name: &str, folder_id: FolderId) -> Option<Folder> {
        info!("Loading folder documents: uri={}", folder_id.uri);

        if !Path::new(folder_id.data.to_system()).is_dir() {
            warn!("Folder path doesn't exist: uri={}", folder_id.data.to_system());
            return None;
        }

        let folder_config = try_load_folder_config(&folder_id);
        let folder_config = Config::merge_opt(folder_config, user_config);

        let parser_settings = ParserSettings::of_config(&Config::or_default(folder_config.clone()));
        let documents = load_docs(&parser_settings, &folder_id);

        Some(Folder::multi_file(name.to_string(), folder_id, documents, folder_config))
    }

    fn update_connection_graph(
        data: &FolderData,
        lookup: &Arc<FolderLookup>,
        change: ConnectionChange,
        previous: &Arc<Conn>,
    ) -> Arc<Conn> {
        let config = data.config_or_default();
        let oracle: Oracle = Arc::new(FolderOracle { data: data.clone(), lookup: Arc::clone(lookup) });

        let conn = if config.core_incremental_references() {
            Conn::update_incremental(previous, &oracle, change)
        } else {
            Conn::mk(&oracle, &data.syms())
        };

        if config.core_paranoid() {
            let rebuilt = Conn::mk(&oracle, &data.syms());
            let diff = Conn::difference(&rebuilt, &conn);
            if !diff.is_empty() {
                panic!("PARANOID MODE ERROR:\n{}", diff.compact_format());
            }
        }

        Arc::new(conn)
    }

    pub fn with_doc(&self, new_doc: Doc) -> Folder {
        match &self.data {
            FolderData::MultiFile(folder) => {
                if new_doc.root_path() != folder.root_path() {
                    panic!(
                        "Updating a folder with an unrelated doc: folder={}; doc={}",
                        folder.root.uri,
                        new_doc.root_path().to_uri()
                    );
                }

                let config = self.data.config_or_default();
                let path = new_doc.rel_path();
                let existing_doc = folder.docs.get(&path).cloned();

                let mut docs = folder.docs.clone();
                docs.insert(path.clone(), new_doc.clone());
                let data = FolderData::MultiFile(Arc::new(MultiFile {
                    name: folder.name.clone(),
                    root: folder.root.clone(),
                    docs,
                    config: folder.config.clone(),
                }));

                let lookup = match &existing_doc {
                    None => Arc::clone(&self.lookup),
                    Some(doc) => Arc::new(self.lookup.without_doc(doc)),
                };
                let lookup = Arc::new(lookup.with_doc(&new_doc));

                let document_change = match &existing_doc {
                    None => DocumentChange::Added(document_input(&new_doc)),
                    Some(existing) => {
                        DocumentChange::Replaced(document_input(existing), document_input(&new_doc))
                    }
                };

                let change = ConnectionChange::of_documents(
                    &config.core_markdown_file_extensions(),
                    &[document_change],
                );

                let conn = if change.is_empty() && !config.core_paranoid() {
                    Arc::clone(&self.conn)
                } else {
                    Folder::update_connection_graph(&data, &lookup, change, &self.conn)
                };

                Folder { data, lookup, conn }
            }
            FolderData::SingleFile(folder) => {
                if new_doc.id() != folder.doc.id() {
                    panic!(
                        "Updating a singleton folder with an unrelated doc: folder={}; doc={}",
                        folder.doc.root_path().to_uri(),
                        new_doc.root_path().to_uri()
                    );
                }
                Folder::mk(FolderData::SingleFile(Arc::new(SingleFile {
                    doc: new_doc,
                    config: folder.config.clone(),
                })))
            }
        }
    }

    pub fn without_doc(&self, doc_id: &DocId) -> Option<Folder> {
        match &self.data {
            FolderData::MultiFile(mf) => {
                let path = doc_id.path().rel_path_forced();
                match mf.docs.get(&path) {
                    None => Some(self.clone()),
                    Some(doc) => {
                        let mut docs = mf.docs.clone();
                        docs.remove(&path);
                        let data = FolderData::MultiFile(Arc::new(MultiFile {
                            name: mf.name.clone(),
                            root: mf.root.clone(),
                            docs,
                            config: mf.config.clone(),
                        }));
                        let lookup = Arc::new(self.lookup.without_doc(doc));

                        let config = data.config_or_default();
                        let change = ConnectionChange::of_documents(
                            &config.core_markdown_file_extensions(),
                            &[DocumentChange::Removed(document_input(doc))],
                        );

                        let conn = Folder::update_connection_graph(
                            &data,
                            &lookup,
                            change,
                            &self.conn,
                        );

                        Some(Folder { data, lookup, conn })
                    }
                }
            }
            FolderData::SingleFile(s) => {
                if s.doc.id() != doc_id {
                    panic!(
                        "Updating a singleton folder with an unrelated doc: folder={}; doc={doc_id}",
                        s.doc.root_path().to_uri()
                    );
                }
                None
            }
        }
    }

    pub fn parser_settings(&self) -> ParserSettings {
        ParserSettings::of_config(&self.config_or_default())
    }

    /// Closing a document reverts it to its on-disk content, or drops it when
    /// the file no longer exists.
    pub fn close_doc(&self, doc_id: &DocId) -> Option<Folder> {
        let parser_settings = self.parser_settings();

        match &self.data {
            FolderData::MultiFile(mf) => {
                let abs = doc_id.path().to_abs();
                match Doc::try_load(&parser_settings, &mf.root, &LocalPath::Abs(abs)) {
                    Some(doc) => Some(self.with_doc(doc)),
                    None => self.without_doc(doc_id),
                }
            }
            FolderData::SingleFile(s) => {
                if s.doc.id() != doc_id {
                    panic!(
                        "Updating a singleton folder with an unrelated doc: folder={}; doc={doc_id}",
                        s.doc.root_path().to_uri()
                    );
                }
                None
            }
        }
    }

    pub fn try_find_doc_by_url(&self, folder_rel_url: &str) -> Option<Doc> {
        let url_encoded = misc::abs_path_url_encode(folder_rel_url);
        self.docs().into_iter().find(|doc| {
            misc::abs_path_url_encode(doc.rel_path().to_system()) == url_encoded
        })
    }

    pub fn doc_count(&self) -> usize {
        match &self.data {
            FolderData::SingleFile(_) => 1,
            FolderData::MultiFile(m) => m.docs.len(),
        }
    }

    pub fn filter_docs_by_slug(&self, slug: &Slug) -> Vec<Doc> {
        self.lookup.filter_docs_by_slug(slug)
    }

    pub fn filter_docs_by_intern_path(&self, path: &InternPath) -> Vec<Doc> {
        let oracle = FolderOracle { data: self.data.clone(), lookup: Arc::clone(&self.lookup) };
        oracle.resolve_documents_by_path(path)
    }

    pub fn filter_docs_by_name(&self, name: &InternName) -> Vec<Doc> {
        let oracle = FolderOracle { data: self.data.clone(), lookup: Arc::clone(&self.lookup) };
        oracle
            .resolve_candidate_documents(name)
            .documents
            .into_iter()
            .map(|id| self.find_doc_by_id(&id))
            .collect()
    }

    pub fn configured_markdown_exts(&self) -> Vec<String> {
        self.config_or_default().core_markdown_file_extensions()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::mk_text;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-folder-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn load(dir: &Path) -> Folder {
        let folder_id =
            FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap()));
        Folder::try_load(None, "test", folder_id).unwrap()
    }

    #[test]
    fn loads_all_markdown_documents_recursively() {
        let dir = temp_dir("load");
        write(&dir, "a.md", "# A\n");
        write(&dir, "sub/b.markdown", "# B\n");
        write(&dir, "sub/c.txt", "not markdown");

        let folder = load(&dir);
        assert_eq!(folder.doc_count(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn honours_gitignore_files() {
        let dir = temp_dir("ignore");
        write(&dir, ".gitignore", "skip/\n");
        write(&dir, "keep.md", "# Keep\n");
        write(&dir, "skip/drop.md", "# Drop\n");

        let folder = load(&dir);
        assert_eq!(folder.doc_count(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn always_skips_vcs_directories() {
        let dir = temp_dir("vcs");
        write(&dir, ".git/notes.md", "# Hidden\n");
        write(&dir, "real.md", "# Real\n");

        let folder = load(&dir);
        assert_eq!(folder.doc_count(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn emacs_backup_files_are_skipped() {
        let dir = temp_dir("emacs");
        write(&dir, ".#a.md", "# Backup\n");
        write(&dir, "a.md", "# A\n");

        let folder = load(&dir);
        assert_eq!(folder.doc_count(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn folder_config_is_read_and_merged() {
        let dir = temp_dir("config");
        write(&dir, ".marksman.toml", "[core.markdown]\nfile_extensions = [\"mdown\"]\n");
        write(&dir, "a.mdown", "# A\n");
        write(&dir, "b.md", "# B\n");

        let folder = load(&dir);
        assert_eq!(folder.doc_count(), 1);
        assert_eq!(folder.configured_markdown_exts(), vec!["mdown".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn documents_are_found_by_slug_and_path_suffix() {
        let dir = temp_dir("lookup");
        write(&dir, "dir/sub/My Note.md", "# My Note\n");
        write(&dir, "other.md", "# Other\n");

        let folder = load(&dir);
        assert_eq!(folder.filter_docs_by_slug(&Slug::of_string("My Note")).len(), 1);

        let name = InternName::mk_unchecked(
            folder.docs()[0].id().clone(),
            "sub/My Note.md".to_string(),
        );
        assert_eq!(folder.filter_docs_by_name(&name).len(), 1);

        let by_title =
            InternName::mk_unchecked(folder.docs()[0].id().clone(), "my-note".to_string());
        assert_eq!(folder.filter_docs_by_name(&by_title).len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn with_doc_updates_indexes_and_graph() {
        let dir = temp_dir("withdoc");
        write(&dir, "a.md", "# A\n");
        write(&dir, "b.md", "# B\n");
        let folder = load(&dir);
        assert_eq!(folder.doc_count(), 2);

        let b = folder.docs().iter().find(|d| d.name() == "B").unwrap().clone();
        let updated = b.with_text(&folder.parser_settings(), mk_text("# Renamed\n"));
        let folder = folder.with_doc(updated);

        assert_eq!(folder.doc_count(), 2);
        assert_eq!(folder.filter_docs_by_slug(&Slug::of_string("Renamed")).len(), 1);
        assert!(folder.filter_docs_by_slug(&Slug::of_string("B")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn without_doc_drops_it_from_lookups() {
        let dir = temp_dir("withoutdoc");
        write(&dir, "a.md", "# A\n");
        write(&dir, "b.md", "# B\n");
        let folder = load(&dir);

        let b = folder.docs().iter().find(|d| d.name() == "B").unwrap().clone();
        let folder = folder.without_doc(b.id()).unwrap();
        assert_eq!(folder.doc_count(), 1);
        assert!(folder.filter_docs_by_slug(&Slug::of_string("B")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn docs_difference_classifies_changes() {
        let dir = temp_dir("diff");
        write(&dir, "a.md", "# A\n");
        write(&dir, "b.md", "# B\n");
        let before = load(&dir);

        write(&dir, "b.md", "# B changed\n");
        write(&dir, "c.md", "# C\n");
        std::fs::remove_file(dir.join("a.md")).unwrap();
        let after = load(&dir);

        let diff = Folder::docs_difference(&before, &after);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.changed.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bogus_folder_is_rejected() {
        let dir = temp_dir("bogus");
        write(&dir, "a.md", "# A\n");
        let folder_id =
            FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap()));
        assert!(!check_workspace_folder_with_warn(&folder_id));

        std::fs::create_dir_all(dir.join(".git")).unwrap();
        assert!(check_workspace_folder_with_warn(&folder_id));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn single_file_folder_holds_one_document() {
        let doc = Doc::mk(
            &ParserSettings::default(),
            DocId::mk_rooted(&FolderId::of_uri("file:///w"), LocalPath::of_system("/w/a.md")),
            Some(1),
            mk_text("# A\n"),
        )
        .unwrap();
        let folder = Folder::single_file(doc.clone(), None);
        assert!(folder.is_single_file());
        assert_eq!(folder.doc_count(), 1);
        assert_eq!(folder.id().uri, doc.uri());
    }

    #[test]
    fn connection_graph_resolves_links_between_loaded_documents() {
        let dir = temp_dir("conn");
        write(&dir, "a.md", "# A\n\n[[B]]\n");
        write(&dir, "b.md", "# B\n");
        let folder = load(&dir);

        let docs = folder.docs();
        let a = docs.iter().find(|d| d.name() == "A").unwrap();
        let b = docs.iter().find(|d| d.name() == "B").unwrap();

        let source = (
            crate::syms::Scope::Doc(a.id().clone()),
            Sym::Ref(crate::syms::Ref::CrossRef(crate::syms::CrossRef::CrossDoc("B".into()))),
        );
        let resolved = folder.conn().resolve(&source);
        assert!(
            resolved
                .iter()
                .any(|(scope, sym)| scope == &crate::syms::Scope::Doc(b.id().clone())
                    && matches!(sym, Sym::Def(Def::Title(t)) if t == "b")),
            "{resolved:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
