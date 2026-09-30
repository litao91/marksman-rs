//! Port of `Tests/SymbolsTests.fs`.

mod common;

use lsp_types::{DocumentSymbol, Location, Range, SymbolInformation, SymbolKind};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::symbols::{self, DocSymbols};
use marksman::workspace::Workspace;

/// `WorkspaceSymbol.mkSymbolInfo`
#[allow(deprecated)]
fn mk_symbol_info(name: &str, kind: SymbolKind, loc: Location) -> SymbolInformation {
    SymbolInformation {
        name: name.to_string(),
        kind,
        location: loc,
        tags: None,
        deprecated: None,
        container_name: None,
    }
}

fn loc(doc: &Doc, range: Range) -> Location {
    Location { uri: marksman::uri::parse(doc.id().uri()), range }
}

// module WorkspaceSymbol

mod workspace_symbol {
    use super::*;

    fn doc1() -> Doc {
        common::FakeDoc::mk_with(&["# A", "#tag1"].join("\n"), "doc1.md", None, None)
    }

    fn doc2() -> Doc {
        common::FakeDoc::mk_with(&["# B", "#tag1 #tag2"].join("\n"), "doc2.md", None, None)
    }

    fn folder() -> Folder {
        common::FakeFolder::mk(vec![doc1(), doc2()])
    }

    fn workspace() -> Workspace {
        Workspace::of_folders(None, vec![folder()])
    }

    #[test]
    fn symbols_no_query() {
        let doc1 = doc1();
        let doc2 = doc2();
        let workspace = workspace();

        let symbols = symbols::workspace_symbols("", &workspace);

        assert_eq!(
            vec![
                mk_symbol_info(
                    "H1: A",
                    SymbolKind::STRING,
                    loc(&doc1, marksman::misc::range(0, 0, 0, 3))
                ),
                mk_symbol_info(
                    "Tag: tag1",
                    SymbolKind::STRING,
                    loc(&doc1, marksman::misc::range(1, 0, 1, 5))
                ),
                mk_symbol_info(
                    "H1: B",
                    SymbolKind::STRING,
                    loc(&doc2, marksman::misc::range(0, 0, 0, 3))
                ),
                mk_symbol_info(
                    "Tag: tag1",
                    SymbolKind::STRING,
                    loc(&doc2, marksman::misc::range(1, 0, 1, 5))
                ),
                mk_symbol_info(
                    "Tag: tag2",
                    SymbolKind::STRING,
                    loc(&doc2, marksman::misc::range(1, 6, 1, 11))
                ),
            ],
            symbols
        );
    }

    #[test]
    fn symbols_with_query() {
        let doc1 = doc1();
        let doc2 = doc2();
        let workspace = workspace();

        let symbols = symbols::workspace_symbols("Tag:", &workspace);

        assert_eq!(
            vec![
                mk_symbol_info(
                    "Tag: tag1",
                    SymbolKind::STRING,
                    loc(&doc1, marksman::misc::range(1, 0, 1, 5))
                ),
                mk_symbol_info(
                    "Tag: tag1",
                    SymbolKind::STRING,
                    loc(&doc2, marksman::misc::range(1, 0, 1, 5))
                ),
                mk_symbol_info(
                    "Tag: tag2",
                    SymbolKind::STRING,
                    loc(&doc2, marksman::misc::range(1, 6, 1, 11))
                ),
            ],
            symbols
        );
    }
}

// module DocSymbols

mod doc_symbols_tests {
    use super::*;

    fn fake_doc() -> Doc {
        common::FakeDoc::mk_lines(&[
            "# E", //
            "## D",
            "#t1",
            "### B",
            "#t2",
            "## C",
            "# A",
        ])
    }

    #[test]
    fn order_no_hierarchy() {
        let fake_doc = fake_doc();
        let syms = symbols::doc_symbols(false, false, &fake_doc);

        let sym_names: Vec<String> = match syms {
            DocSymbols::Flat(x) => x,
            _ => panic!("Unexpected symbol type"),
        }
        .into_iter()
        .map(|x| x.name)
        .collect();

        assert_eq!(
            vec![
                "H1: E".to_string(),
                "H2: D".to_string(),
                "H3: B".to_string(),
                "H2: C".to_string(),
                "H1: A".to_string(),
                "Tag: t1".to_string(),
                "Tag: t2".to_string(),
            ],
            sym_names
        );
    }

    #[test]
    fn order_hierarchy() {
        let fake_doc = fake_doc();
        let syms = symbols::doc_symbols(true, false, &fake_doc);

        let syms = match syms {
            DocSymbols::Hierarchical(x) => x,
            _ => panic!("Unexpected symbol type"),
        };

        let mut names: Vec<String> = Vec::new();

        fn collect(sym: &DocumentSymbol, names: &mut Vec<String>) {
            names.push(sym.name.clone());
            for child in sym.children.as_ref().map(|c| c.as_slice()).unwrap_or(&[]) {
                collect(child, names);
            }
        }

        for sym in &syms {
            collect(sym, &mut names);
        }

        assert_eq!(
            vec![
                "E".to_string(),
                "D".to_string(),
                "t1".to_string(),
                "B".to_string(),
                "t2".to_string(),
                "C".to_string(),
                "A".to_string()
            ],
            names
        );
    }
}
