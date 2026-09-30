//! Document and workspace symbols.
//!
//! Port of `Marksman.Symbols`.

use lsp_types::{DocumentSymbol, Location, SymbolInformation, SymbolKind, Uri};

use crate::cst::{Cst, Element, Node, Tag, Heading};
use crate::doc::Doc;
use crate::misc::is_subsequence_of;
use crate::names::DocId;
use crate::workspace::Workspace;

pub fn heading_to_symbol_name(h: &Node<Heading>) -> String {
    format!("H{}: {}", h.data.level, h.data.name())
}

pub fn tag_to_symbol_name(t: &Node<Tag>) -> String {
    format!("Tag: {}", t.data.name.text)
}

fn url_of(uri: &str) -> Uri {
    crate::uri::parse(uri)
}

#[allow(deprecated)]
pub fn tag_to_symbol_info(doc_uri: &DocId, t: &Node<Tag>) -> SymbolInformation {
    SymbolInformation {
        name: tag_to_symbol_name(t),
        kind: SymbolKind::STRING,
        tags: None,
        deprecated: None,
        location: Location { uri: url_of(doc_uri.uri()), range: t.range },
        container_name: None,
    }
}

#[allow(deprecated)]
pub fn heading_to_symbol_info(doc_uri: &DocId, h: &Node<Heading>) -> SymbolInformation {
    SymbolInformation {
        name: heading_to_symbol_name(h),
        kind: SymbolKind::STRING,
        tags: None,
        deprecated: None,
        location: Location { uri: url_of(doc_uri.uri()), range: h.range },
        container_name: None,
    }
}

#[allow(deprecated)]
pub fn tag_to_document_symbol(t: &Node<Tag>) -> DocumentSymbol {
    DocumentSymbol {
        name: t.data.name.text.clone(),
        detail: None,
        kind: SymbolKind::STRING,
        range: t.range,
        selection_range: t.range,
        children: None,
        tags: None,
        deprecated: None,
    }
}

#[allow(deprecated)]
pub fn heading_to_document_symbol(is_emacs: bool, cst: &Cst, h: &Node<Heading>) -> DocumentSymbol {
    let name = h.data.name().to_string();
    let kind = SymbolKind::STRING;
    let range = h.data.scope();
    let selection_range = h.data.range();

    let children: Vec<DocumentSymbol> = cst
        .children(&Element::H(h.clone()))
        .into_iter()
        .filter_map(|e| match e {
            Element::H(h) => Some(heading_to_document_symbol(is_emacs, cst, &h)),
            Element::T(t) => Some(tag_to_document_symbol(&t)),
            _ => None,
        })
        .collect();

    let children = if children.is_empty() {
        None
    } else if is_emacs {
        // Emacs' imenu with consult/counsel/etc. doesn't allow selecting
        // intermediate nodes that have children. As a workaround we add a '.'
        // to this node.
        let this_heading = DocumentSymbol {
            name: ".".to_string(),
            detail: None,
            kind,
            range: selection_range,
            selection_range,
            children: None,
            tags: None,
            deprecated: None,
        };
        let mut with_self = vec![this_heading];
        with_self.extend(children);
        Some(with_self)
    } else {
        Some(children)
    };

    DocumentSymbol {
        name,
        detail: None,
        kind,
        range,
        selection_range,
        children,
        tags: None,
        deprecated: None,
    }
}

pub enum DocSymbols {
    Flat(Vec<SymbolInformation>),
    Hierarchical(Vec<DocumentSymbol>),
}

pub fn doc_symbols(hierarchy: bool, is_emacs: bool, doc: &Doc) -> DocSymbols {
    if hierarchy {
        let cst = doc.cst();
        let top_level = cst.top_level_headings();
        DocSymbols::Hierarchical(
            top_level
                .iter()
                .map(|h| heading_to_document_symbol(is_emacs, cst, h))
                .collect(),
        )
    } else {
        let mut all: Vec<SymbolInformation> = doc
            .index()
            .headings
            .iter()
            .map(|h| heading_to_symbol_info(doc.id(), h))
            .collect();
        all.extend(
            doc.index()
                .tags
                .iter()
                .map(|t| tag_to_symbol_info(doc.id(), t)),
        );
        DocSymbols::Flat(all)
    }
}

pub fn workspace_symbols(query: &str, ws: &Workspace) -> Vec<SymbolInformation> {
    let mut out = Vec::new();
    for folder in ws.folders() {
        for doc in folder.docs() {
            let index = doc.index();
            out.extend(
                index
                    .headings
                    .iter()
                    .filter(|h| is_subsequence_of(query, &heading_to_symbol_name(h)))
                    .map(|h| heading_to_symbol_info(doc.id(), h)),
            );
            out.extend(
                index
                    .tags
                    .iter()
                    .filter(|t| is_subsequence_of(query, &tag_to_symbol_name(t)))
                    .map(|t| tag_to_symbol_info(doc.id(), t)),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::folder::Folder;
    use crate::names::FolderId;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-symbols-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn load(dir: &std::path::Path) -> Folder {
        let id = FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap()));
        Folder::try_load(None, "test", id).unwrap()
    }

    #[test]
    fn symbol_names_carry_their_kind() {
        let dir = temp_dir("names");
        write(&dir, "a.md", "# Title\n\nbody #tag\n");
        let folder = load(&dir);
        let doc = folder.docs().into_iter().next().unwrap();

        assert_eq!(heading_to_symbol_name(&doc.index().headings[0]), "H1: Title");
        assert_eq!(tag_to_symbol_name(&doc.index().tags[0]), "Tag: tag");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn flat_symbols_list_headings_then_tags() {
        let dir = temp_dir("flat");
        write(&dir, "a.md", "# Title\n\n## Sub\n\nbody #tag\n");
        let folder = load(&dir);
        let doc = folder.docs().into_iter().next().unwrap();

        match doc_symbols(false, false, &doc) {
            DocSymbols::Flat(syms) => {
                assert_eq!(syms.len(), 3);
                assert_eq!(syms[0].name, "H1: Title");
                assert_eq!(syms[1].name, "H2: Sub");
                assert_eq!(syms[2].name, "Tag: tag");
                assert_eq!(syms[0].location.uri.as_str(), doc.uri());
            }
            DocSymbols::Hierarchical(_) => panic!("expected flat symbols"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hierarchical_symbols_nest_by_heading_level() {
        let dir = temp_dir("hier");
        write(&dir, "a.md", "# T1\n\n## T2\n\n### T3\n\n## T4\n");
        let folder = load(&dir);
        let doc = folder.docs().into_iter().next().unwrap();

        match doc_symbols(true, false, &doc) {
            DocSymbols::Hierarchical(syms) => {
                assert_eq!(syms.len(), 1);
                assert_eq!(syms[0].name, "T1");
                let children = syms[0].children.clone().unwrap();
                assert_eq!(children.len(), 2);
                assert_eq!(children[0].name, "T2");
                assert_eq!(children[0].children.as_ref().unwrap()[0].name, "T3");
                assert_eq!(children[1].name, "T4");
            }
            DocSymbols::Flat(_) => panic!("expected hierarchical symbols"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn emacs_gets_a_selectable_placeholder_child() {
        let dir = temp_dir("emacs");
        write(&dir, "a.md", "# T1\n\n## T2\n");
        let folder = load(&dir);
        let doc = folder.docs().into_iter().next().unwrap();

        match doc_symbols(true, true, &doc) {
            DocSymbols::Hierarchical(syms) => {
                let children = syms[0].children.clone().unwrap();
                assert_eq!(children[0].name, ".");
                assert_eq!(children[1].name, "T2");
            }
            DocSymbols::Flat(_) => panic!("expected hierarchical symbols"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn multiple_top_level_headings_stay_siblings() {
        let dir = temp_dir("toplevel");
        write(&dir, "a.md", "## L2\n# L1\n## L3\n");
        let folder = load(&dir);
        let doc = folder.docs().into_iter().next().unwrap();

        match doc_symbols(true, false, &doc) {
            DocSymbols::Hierarchical(syms) => {
                let names: Vec<&str> = syms.iter().map(|s| s.name.as_str()).collect();
                assert_eq!(names, vec!["L2", "L1"]);
            }
            DocSymbols::Flat(_) => panic!("expected hierarchical symbols"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn workspace_symbols_match_by_subsequence() {
        let dir = temp_dir("wssym");
        write(&dir, "a.md", "# Alpha Beta\n\ntext #important\n");
        write(&dir, "b.md", "# Gamma\n");
        let folder = load(&dir);
        let ws = Workspace::of_folders(None, vec![folder]);

        let found = workspace_symbols("ab", &ws);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "H1: Alpha Beta");

        let tags = workspace_symbols("imp", &ws);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "Tag: important");

        assert!(workspace_symbols("zzz", &ws).is_empty());
        let _ = Config::default_config();
        std::fs::remove_dir_all(&dir).ok();
    }
}
