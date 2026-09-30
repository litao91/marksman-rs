//! Port of `Tests/RefsTests.fs`.

mod common;

use lsp_types::Position;

use marksman::config::{ComplWikiStyle, Config};
use marksman::cst::{self, Element};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::misc;
use marksman::names::{DocId, InternName};
use marksman::refs::{detect_file_link_kind, Dest, FileLink, FileLinkKind};

use common::{dummy_root_path, mk_doc_id, mk_folder_id, FakeDoc, FakeFolder};

/// `InternName.tryAsPath >> Option.map (InternPath.toRel >> RelPath.toSystem)`
fn intern_as_path(doc_id: &DocId, name: &str) -> Option<String> {
    InternName::mk_unchecked(doc_id.clone(), name.to_string())
        .try_as_path()
        .map(|p| p.to_rel().to_system().to_string())
}

/// `requireElementAtPos`
fn require_element_at_pos(doc: &Doc, line: u32, col: u32) -> Element {
    doc.cst()
        .element_at_pos(Position { line, character: col })
        .unwrap_or_else(|| panic!("No element found at ({line}, {col})"))
        .clone()
}

/// `formatRefs`: `(Path.GetFileName(Doc.uri doc), el.Range.DebuggerDisplay)`
fn format_refs(refs: &[(Doc, Element)]) -> Vec<String> {
    refs.iter()
        .map(|(doc, el)| {
            format!("({}, {})", misc::file_name(doc.uri()), cst::fmt_range(el.range()))
        })
        .collect()
}

/// `checkInlineSnapshot`: the F# helper splits every formatted item into lines
/// and compares the concatenated sequence against the inline expectation.
fn check_inline_snapshot(actual: &[String], expected: &[&str]) {
    let expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
    assert_eq!(actual, &expected);
}

/// `FakeDoc.Mk(path = ..., contentLines = [| ... |])`
fn doc_lines(path: &str, lines: &[&str]) -> Doc {
    FakeDoc::mk_at(&lines.join("\n"), path)
}

/// `{ Config.Default with ... }`
fn config_with(f: impl FnOnce(&mut Config)) -> Config {
    let mut config = Config::default_config();
    f(&mut config);
    config
}

mod intern_name_tests {
    use super::*;

    #[test]
    fn rel_path_1() {
        let folder = mk_folder_id(&dummy_root_path(&["rootFolder"]));

        let doc_path =
            mk_doc_id(&folder, &dummy_root_path(&["rootFolder", "subfolder", "sub.md"]));

        let actual = intern_as_path(&doc_path, "../doc.md").unwrap();

        assert_eq!("doc.md", actual);
    }

    #[test]
    fn rel_path_2() {
        let folder = mk_folder_id(&dummy_root_path(&["rootFolder"]));
        let doc_path = mk_doc_id(&folder, &dummy_root_path(&["rootFolder", "doc1.md"]));
        let actual = intern_as_path(&doc_path, "./doc2.md").unwrap();

        assert_eq!("doc2.md", actual);
    }

    #[test]
    fn rel_path_non_exist() {
        let folder = mk_folder_id(&dummy_root_path(&["rootFolder"]));
        let doc_path = mk_doc_id(&folder, &dummy_root_path(&["rootFolder", "doc1.md"]));
        let actual = intern_as_path(&doc_path, "../doc2.md");
        assert_eq!(None, actual);
    }

    #[test]
    fn root_path() {
        let folder = mk_folder_id(&dummy_root_path(&["rootFolder"]));

        let doc_path =
            mk_doc_id(&folder, &dummy_root_path(&["rootFolder", "subfolder", "sub.md"]));

        let actual = intern_as_path(&doc_path, "/doc.md").unwrap();

        assert_eq!("doc.md", actual);
    }

    #[test]
    fn url_no_schema_fp() {
        let folder = mk_folder_id(&dummy_root_path(&["rootFolder"]));

        let doc_path =
            mk_doc_id(&folder, &dummy_root_path(&["rootFolder", "subfolder", "sub.md"]));

        let actual = intern_as_path(&doc_path, "www.google.com").unwrap();

        assert_eq!("www.google.com", actual);
    }

    #[test]
    fn url_schema() {
        let folder = mk_folder_id(&dummy_root_path(&["rootFolder"]));

        let doc_path =
            mk_doc_id(&folder, &dummy_root_path(&["rootFolder", "subfolder", "sub.md"]));

        let actual = intern_as_path(&doc_path, "http://www.google.com");

        assert_eq!(None, actual);
    }
}

mod file_link_tests {
    use super::*;

    fn doc1() -> Doc {
        doc_lines("doc1.md", &["# Horses"])
    }

    fn sub_doc1() -> Doc {
        doc_lines("sub/doc1.md", &["# Sub Horses"])
    }

    fn sub_sub_doc1() -> Doc {
        doc_lines("sub/sub/doc1.md", &["# Sub Sub Horses"])
    }

    fn doc2() -> Doc {
        doc_lines("doc2.md", &["# Riding Horses"])
    }

    fn sub_doc2() -> Doc {
        doc_lines("sub/doc2.md", &["# Sub Riding Horses"])
    }

    fn doc3() -> Doc {
        doc_lines("doc3.md", &["# Fruit"])
    }

    fn doc4() -> Doc {
        doc_lines("file with spaces.md", &["# File with spaces"])
    }

    #[test]
    fn file_name_partial() {
        let (doc1, doc2, doc4) = (doc1(), doc2(), doc4());
        let folder = FakeFolder::mk_with_config(
            vec![doc1.clone(), doc2.clone(), doc4.clone()],
            Some(config_with(|c| c.compl_wiki_style = Some(ComplWikiStyle::FileStem))),
        );

        let partial =
            FileLink::filter_matching_docs(&folder, &InternName::mk_unchecked(doc1.id().clone(), "doc"));

        assert!(partial.is_empty());

        let full =
            FileLink::filter_matching_docs(&folder, &InternName::mk_unchecked(doc1.id().clone(), "doc2"));

        assert_eq!(1, full.len());
        assert_eq!(doc2, full[0].doc);

        let full_with_spaces_encoded = FileLink::filter_matching_docs(
            &folder,
            &InternName::mk_unchecked(doc2.id().clone(), "file%20with%20spaces.md"),
        );

        assert_eq!(1, full_with_spaces_encoded.len());
        assert_eq!(doc4, full_with_spaces_encoded[0].doc);

        let full_with_spaces_not_encoded = FileLink::filter_matching_docs(
            &folder,
            &InternName::mk_unchecked(doc2.id().clone(), "file with spaces.md"),
        );

        assert_eq!(1, full_with_spaces_not_encoded.len());
        assert_eq!(doc4, full_with_spaces_not_encoded[0].doc);
    }

    #[test]
    fn file_name_relative_as_abs() {
        let (doc1, sub_doc1, sub_sub_doc1, doc2, sub_doc2) =
            (doc1(), sub_doc1(), sub_sub_doc1(), doc2(), sub_doc2());
        let folder = FakeFolder::mk_with_config(
            vec![
                doc1.clone(),
                sub_doc1.clone(),
                sub_sub_doc1.clone(),
                doc2.clone(),
                sub_doc2.clone(),
            ],
            Some(config_with(|c| c.compl_wiki_style = Some(ComplWikiStyle::FilePathStem))),
        );

        let actual: Vec<Doc> =
            FileLink::filter_matching_docs(&folder, &InternName::mk_unchecked(doc1.id().clone(), "doc1"))
                .into_iter()
                .map(|fl| fl.doc)
                .collect();

        assert_eq!(actual, vec![doc1.clone(), sub_doc1.clone(), sub_sub_doc1.clone()]);

        let actual: Vec<Doc> = FileLink::filter_matching_docs(
            &folder,
            &InternName::mk_unchecked(sub_doc1.id().clone(), "doc2"),
        )
        .into_iter()
        .map(|fl| fl.doc)
        .collect();

        assert_eq!(actual, vec![doc2.clone(), sub_doc2.clone()]);

        let actual: Vec<Doc> = FileLink::filter_matching_docs(
            &folder,
            &InternName::mk_unchecked(doc1.id().clone(), "sub/doc1"),
        )
        .into_iter()
        .map(|fl| fl.doc)
        .collect();

        assert_eq!(actual, vec![sub_doc1, sub_sub_doc1]);
    }

    #[test]
    fn heading_partial() {
        let (doc1, doc2, doc3) = (doc1(), doc2(), doc3());
        let folder = FakeFolder::mk_with_config(
            vec![doc1.clone(), doc2.clone(), doc3.clone()],
            Some(config_with(|c| c.compl_wiki_style = Some(ComplWikiStyle::TitleSlug))),
        );

        let actual: Vec<Doc> =
            FileLink::filter_matching_docs(&folder, &InternName::mk_unchecked(doc3.id().clone(), "horses"))
                .into_iter()
                .map(|fl| fl.doc)
                .collect();

        assert_eq!(vec![doc1], actual);

        let actual: Vec<Doc> = FileLink::filter_matching_docs(
            &folder,
            &InternName::mk_unchecked(doc3.id().clone(), "riding-horses"),
        )
        .into_iter()
        .map(|fl| fl.doc)
        .collect();

        assert_eq!(vec![doc2], actual);
    }

    #[test]
    fn title_similar_to_name() {
        let d1 = FakeDoc::mk_at("# Doc1 idx\n\n## Sub", "doc1.md");
        let d2 = FakeDoc::mk_at("[[doc1-idx#sub]]", "test.md");

        let link_kind = detect_file_link_kind(ComplWikiStyle::FilePathStem, d2.id(), "doc1-idx", &d1);

        assert_eq!(FileLinkKind::Title, link_kind);
    }
}

mod basic_refs_tests {
    use super::*;

    fn doc1() -> Doc {
        doc_lines(
            "doc1.md",
            &[
                "# Doc 1",           // 0
                "",                  // 1
                "## D1 H2.1",        // 2
                "",                  // 3
                "[[doc-2#d2-h22]]",  // 4
                "",                  // 5
                "## D1 H2.2",        // 6
                "",                  // 7
                "[[#dup]]",          // 8
                "",                  // 9
                "## Dup",            // 10
                "Entry 1",           // 11
                "",                  // 12
                "## Dup",            // 13
                "Entry 2",           // 14
                "",                  // 15
                "#tag1 #tag2",       // 16
            ],
        )
    }

    fn doc2() -> Doc {
        doc_lines(
            "doc2.md",
            &[
                "# Doc 2",                    // 0
                "",                           // 1
                "# D2 H2.1",                  // 2
                "",                           // 3
                "[D2-Link-1]",                // 4
                "",                           // 5
                "[[#d2-h22]]",                // 6
                "",                           // 7
                "[d2-LINK-1]",                // 8
                "",                           // 9
                "# D2 H2.2",                  // 10
                "",                           // 11
                "[[doc-1]]",                  // 12
                "[[doc-1#dup]]",              // 13
                "[lbl1](/doc1.md)",           // 14
                "[^fn1]",                     // 15
                "",                           // 16
                "[d2-link-1]: some-url",      // 17
                "",                           // 18
                "[^fn1]: This is footnote",   // 19
                "",                           // 20
                "#tag1",                      // 21
            ],
        )
    }

    fn fixtures() -> (Doc, Doc, Folder) {
        let doc1 = doc1();
        let doc2 = doc2();
        let folder = FakeFolder::mk(vec![doc1.clone(), doc2.clone()]);
        (doc1, doc2, folder)
    }

    #[test]
    fn ref_to_tag_at_tag() {
        let (_doc1, doc2, folder) = fixtures();
        let def = require_element_at_pos(&doc2, 21, 1);

        let refs = format_refs(&Dest::find_element_refs(false, &folder, &doc2, &def));

        check_inline_snapshot(&refs, &["(doc1.md, (16,0)-(16,5))"]);
    }

    #[test]
    fn ref_to_tag_at_tag_with_decl() {
        let (_doc1, doc2, folder) = fixtures();
        let def = require_element_at_pos(&doc2, 21, 1);

        let refs = format_refs(&Dest::find_element_refs(true, &folder, &doc2, &def));

        check_inline_snapshot(
            &refs,
            &["(doc1.md, (16,0)-(16,5))", "(doc2.md, (21,0)-(21,5))"],
        );
    }

    #[test]
    fn ref_to_link_def_at_def() {
        let (_doc1, doc2, folder) = fixtures();
        let def = require_element_at_pos(&doc2, 17, 3);

        let refs = format_refs(&Dest::find_element_refs(false, &folder, &doc2, &def));

        check_inline_snapshot(
            &refs,
            &["(doc2.md, (4,0)-(4,11))", "(doc2.md, (8,0)-(8,11))"],
        );
    }

    #[test]
    fn ref_to_link_def_at_def_with_decl() {
        let (_doc1, doc2, folder) = fixtures();
        let def = require_element_at_pos(&doc2, 17, 3);

        let refs = format_refs(&Dest::find_element_refs(true, &folder, &doc2, &def));

        check_inline_snapshot(
            &refs,
            &[
                "(doc2.md, (4,0)-(4,11))",
                "(doc2.md, (8,0)-(8,11))",
                "(doc2.md, (17,0)-(17,21))",
            ],
        );
    }

    #[test]
    fn ref_to_link_def_at_link() {
        let (_doc1, doc2, folder) = fixtures();
        let def = require_element_at_pos(&doc2, 8, 4);

        let refs = format_refs(&Dest::find_element_refs(false, &folder, &doc2, &def));

        check_inline_snapshot(
            &refs,
            &["(doc2.md, (4,0)-(4,11))", "(doc2.md, (8,0)-(8,11))"],
        );
    }

    #[test]
    fn ref_to_link_def_at_link_with_decl() {
        let (_doc1, doc2, folder) = fixtures();
        let def = require_element_at_pos(&doc2, 8, 4);

        let refs = format_refs(&Dest::find_element_refs(true, &folder, &doc2, &def));

        check_inline_snapshot(
            &refs,
            &[
                "(doc2.md, (4,0)-(4,11))",
                "(doc2.md, (8,0)-(8,11))",
                "(doc2.md, (17,0)-(17,21))",
            ],
        );
    }

    #[test]
    #[ignore = "Skipped in the original too: `Fact(Skip = \"Footnote parsing not implemented\")`. \
                F# expectation: [(doc2.md, (19,0)-(19,16)); (doc2.md, (15,0)-(15,6))]"]
    fn ref_to_footnote_at_link() {
        let (_doc1, doc2, folder) = fixtures();
        let fn_link = require_element_at_pos(&doc2, 15, 2);

        let refs = format_refs(&Dest::find_element_refs(true, &folder, &doc2, &fn_link));

        check_inline_snapshot(
            &refs,
            &["(doc2.md, (19,0)-(19,16))", "(doc2.md, (15,0)-(15,6))"],
        );
    }

    #[test]
    fn ref_to_doc_at_title() {
        let (doc1, _doc2, folder) = fixtures();
        let title = require_element_at_pos(&doc1, 0, 2);

        let refs = format_refs(&Dest::find_element_refs(false, &folder, &doc1, &title));

        check_inline_snapshot(
            &refs,
            &[
                "(doc2.md, (12,0)-(12,9))",
                "(doc2.md, (13,0)-(13,13))",
                "(doc2.md, (14,0)-(14,16))",
            ],
        );
    }

    #[test]
    fn ref_to_doc_at_title_with_decl() {
        let (doc1, _doc2, folder) = fixtures();
        let title = require_element_at_pos(&doc1, 0, 2);

        let refs = format_refs(&Dest::find_element_refs(true, &folder, &doc1, &title));

        check_inline_snapshot(
            &refs,
            &[
                "(doc1.md, (0,0)-(0,7))",
                "(doc2.md, (12,0)-(12,9))",
                "(doc2.md, (13,0)-(13,13))",
                "(doc2.md, (14,0)-(14,16))",
            ],
        );
    }

    #[test]
    fn ref_to_doc_at_link() {
        let (doc1, _doc2, folder) = fixtures();
        let wl = require_element_at_pos(&doc1, 4, 4);

        let refs = format_refs(&Dest::find_element_refs(false, &folder, &doc1, &wl));

        check_inline_snapshot(
            &refs,
            &["(doc1.md, (4,0)-(4,16))", "(doc2.md, (6,0)-(6,11))"],
        );
    }

    #[test]
    fn ref_to_doc_at_link_with_decl() {
        let (doc1, _doc2, folder) = fixtures();
        let wl = require_element_at_pos(&doc1, 4, 4);

        let refs = format_refs(&Dest::find_element_refs(true, &folder, &doc1, &wl));

        check_inline_snapshot(
            &refs,
            &[
                "(doc1.md, (4,0)-(4,16))",
                "(doc2.md, (6,0)-(6,11))",
                "(doc2.md, (10,0)-(10,9))",
            ],
        );
    }
}

mod link_kind_refs_tests {
    use super::*;

    fn doc1() -> Doc {
        doc_lines(
            "file1.md",
            &[
                "# Doc 1 Title",
                "[[file2]]",
                "[[file2.md]]",
                "[[file2#doc-2-subtitle]]",
                "[[doc-2-title]]",
                "[link](file2)",
                "[[file3]]",
            ],
        )
    }

    fn doc2() -> Doc {
        doc_lines("file2.md", &["# Doc 2 Title", "## Doc 2 SubTitle", "[[doc-3-title]]"])
    }

    fn doc3() -> Doc {
        doc_lines("sub/file3.md", &["# Doc 3 Title"])
    }

    fn fixtures() -> (Doc, Doc, Folder) {
        let doc1 = doc1();
        let doc2 = doc2();
        let doc3 = doc3();
        let folder = FakeFolder::mk(vec![doc1.clone(), doc2.clone(), doc3]);
        (doc1, doc2, folder)
    }

    #[test]
    fn at_wiki_various_filenames() {
        let (doc1, _doc2, folder) = fixtures();
        let link = require_element_at_pos(&doc1, 1, 2);
        let refs = format_refs(&Dest::find_element_refs(false, &folder, &doc1, &link));

        check_inline_snapshot(
            &refs,
            &[
                "(file1.md, (1,0)-(1,9))",
                "(file1.md, (2,0)-(2,12))",
                // Referencing a doc should resolve to all sections in the doc
                "(file1.md, (3,0)-(3,24))",
                "(file1.md, (4,0)-(4,15))",
                "(file1.md, (5,0)-(5,13))",
            ],
        );
    }

    #[test]
    fn at_wiki_filenames_subfolder() {
        let (_doc1, doc2, folder) = fixtures();
        let link = require_element_at_pos(&doc2, 2, 3);
        let refs = format_refs(&Dest::find_element_refs(false, &folder, &doc2, &link));

        check_inline_snapshot(
            &refs,
            &["(file1.md, (6,0)-(6,9))", "(file2.md, (2,0)-(2,15))"],
        );
    }
}

mod encoding_tests {
    use super::*;

    const DOC1_CONTENT: &str = r#"# Doc 1

## Heading 1

## Heading 2

[[#Heading 1]]
[[#Heading%201]]
[[Doc 2]]
[[Doc%202]]
[[Doc 2#Heading %231]]
[](Doc%202)
[](Doc%202#heading-1)
[[doc.3.with.dots.md]]
[[doc.3.with.dots]]
"#;

    const DOC2_CONTENT: &str = r#"# Doc 2

## Heading #1
"#;

    fn doc1() -> Doc {
        FakeDoc::mk_at(DOC1_CONTENT, "doc1.md")
    }

    fn doc2() -> Doc {
        FakeDoc::mk_at(DOC2_CONTENT, "doc2.md")
    }

    fn doc3() -> Doc {
        FakeDoc::mk_at("Doc 3", "doc.3.with.dots.md")
    }

    fn fixtures() -> (Doc, Folder) {
        let doc1 = doc1();
        let folder = FakeFolder::mk_with_config(
            vec![doc1.clone(), doc2(), doc3()],
            Some(config_with(|c| c.compl_wiki_style = Some(ComplWikiStyle::FilePathStem))),
        );
        (doc1, folder)
    }

    fn simplify_dest(dest: &Dest) -> String {
        match dest {
            Dest::Doc(file_link) => file_link.doc.name(),
            Dest::Heading(doc_link, node) => {
                format!("{} / {}", doc_link.doc().name(), node.text)
            }
            Dest::LinkDef(doc, node) => {
                let def_name = node.data.name();
                format!("{} / {def_name}", doc.name())
            }
            Dest::Tag(doc, node) => {
                let tag = &node.text;
                format!("{} / {tag}", doc.name())
            }
        }
    }

    fn resolve_at_pos(folder: &Folder, doc: &Doc, line: u32, col: u32) -> Vec<String> {
        let el = require_element_at_pos(doc, line, col);
        Dest::try_resolve_element(folder, doc, &el)
            .iter()
            .map(simplify_dest)
            .collect()
    }

    #[test]
    fn heading_not_encoding() {
        let (doc1, folder) = fixtures();
        let refs = resolve_at_pos(&folder, &doc1, 6, 5);
        check_inline_snapshot(&refs, &["Doc 1 / ## Heading 1"]);
    }

    #[test]
    fn heading_url_encoded() {
        let (doc1, folder) = fixtures();
        let refs = resolve_at_pos(&folder, &doc1, 7, 5);
        check_inline_snapshot(&refs, &["Doc 1 / ## Heading 1"]);
    }

    #[test]
    fn doc_not_encoded() {
        let (doc1, folder) = fixtures();
        let refs = resolve_at_pos(&folder, &doc1, 8, 5);
        check_inline_snapshot(&refs, &["Doc 2 / # Doc 2"]);
    }

    #[test]
    fn doc_url_encoded() {
        let (doc1, folder) = fixtures();
        let refs = resolve_at_pos(&folder, &doc1, 9, 5);
        check_inline_snapshot(&refs, &["Doc 2 / # Doc 2"]);
    }

    #[test]
    fn doc_heading_mixed_encoding() {
        let (doc1, folder) = fixtures();
        let refs = resolve_at_pos(&folder, &doc1, 10, 5);
        check_inline_snapshot(&refs, &["Doc 2 / ## Heading #1"]);
    }

    #[test]
    fn inline_doc_url_encoding() {
        let (doc1, folder) = fixtures();
        let refs = resolve_at_pos(&folder, &doc1, 11, 5);
        check_inline_snapshot(&refs, &["Doc 2 / # Doc 2"]);
    }

    #[test]
    fn inline_doc_heading_mixed_encoding() {
        let (doc1, folder) = fixtures();
        let refs = resolve_at_pos(&folder, &doc1, 12, 5);
        check_inline_snapshot(&refs, &["Doc 2 / ## Heading #1"]);
    }

    #[test]
    fn doc_file_name_with_dots() {
        let (doc1, folder) = fixtures();
        let refs = resolve_at_pos(&folder, &doc1, 13, 5);
        check_inline_snapshot(&refs, &["doc.3.with.dots"]);

        let refs = resolve_at_pos(&folder, &doc1, 14, 5);
        check_inline_snapshot(&refs, &["doc.3.with.dots"]);
    }
}

/// Cases where title_from_heading = false
mod title_less {
    use super::*;

    #[test]
    fn ref_to_h1() {
        let base_config =
            config_with(|c| c.compl_wiki_style = Some(ComplWikiStyle::TitleSlug));

        let mk_folder = |config: &Config| {
            let d1 = FakeDoc::mk_with("# Doc1", "d1.md", None, Some(config.clone()));
            //                                                             012345
            let d2 = FakeDoc::mk_with("[[Doc1]]", "d2.md", None, Some(config.clone()));

            let folder =
                FakeFolder::mk_with_config(vec![d1.clone(), d2.clone()], Some(config.clone()));
            (d1, d2, folder)
        };

        // In title-less mode headings are not referenceable cross-doc
        let (_d1, d2, folder) =
            mk_folder(&Config { core_title_from_heading: Some(false), ..base_config.clone() });

        let el = require_element_at_pos(&d2, 0, 2);
        assert!(Dest::try_resolve_element(&folder, &d2, &el).is_empty());

        // In title-full mode headings *are* referenceable cross-doc
        let (_d1, d2, folder) =
            mk_folder(&Config { core_title_from_heading: Some(true), ..base_config });

        assert!(!Dest::try_resolve_element(&folder, &d2, &el).is_empty());
    }
}

mod regression_tests {
    use super::*;

    #[test]
    fn root_link_issue275() {
        let doc = doc_lines("doc.md", &["[](/)"]);
        let folder = FakeFolder::mk(vec![doc.clone()]);
        let el = require_element_at_pos(&doc, 0, 4);
        assert!(Dest::try_resolve_element(&folder, &doc, &el).is_empty());
    }
}
