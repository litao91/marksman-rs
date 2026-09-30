//! Port of `Tests/RefactorTests.fs`.

mod common;

use std::collections::BTreeMap;

use lsp_types::{DocumentChanges, OneOf, Position, Range};
use marksman::misc;
use marksman::refactor::{self, RenameResult};

/// `FakeDoc.Mk(contentLines, path = ...)`.
fn fake_doc(lines: &[&str], path: &str) -> marksman::doc::Doc {
    common::FakeDoc::mk_with(&lines.join("\n"), path, None, None)
}

/// Mirrors the `editsByFile` helper at the top of the original file: the
/// workspace edit is projected onto `fileName -> (range, newText)` pairs.
fn edits_by_file(res: &RenameResult) -> BTreeMap<String, Vec<(Range, String)>> {
    match res {
        RenameResult::Edit(ws_edit) => match &ws_edit.document_changes {
            Some(DocumentChanges::Edits(doc_changes)) => doc_changes
                .iter()
                .map(|doc_edit| {
                    let doc = marksman::misc::file_name(doc_edit.text_document.uri.as_str())
                        .to_string();
                    let ranges = doc_edit
                        .edits
                        .iter()
                        .map(|x| match x {
                            OneOf::Left(edit) => (edit.range, edit.new_text.clone()),
                            _ => panic!("Refactoring should always produce TextDocumentEdits"),
                        })
                        .collect();
                    (doc, ranges)
                })
                .collect(),
            Some(DocumentChanges::Operations(_)) => {
                panic!("Refactoring should always produce TextDocumentEdits")
            }
            None => match &ws_edit.changes {
                Some(doc_edit_map) => doc_edit_map
                    .iter()
                    .map(|(doc, edits)| {
                        let ranges =
                            edits.iter().map(|x| (x.range, x.new_text.clone())).collect();
                        (misc::file_name(doc.as_str()).to_string(), ranges)
                    })
                    .collect(),
                None => BTreeMap::new(),
            },
        },
        other => panic!("Edit ranges are not defined for: {other:?}"),
    }
}

// module RenameTests
//   module ReferenceLinks

mod reference_links {
    use super::*;

    pub(super) fn mk_workspace() -> (marksman::doc::Doc, marksman::folder::Folder) {
        let doc1 = fake_doc(
            //  0         1         2
            //  0123456789012345678901234567890
            &[
                "# Doc 1",
                "Start [lbl1], then [lbl2].",
                "Then [lbl1] again.",
                "And [broken] label.",
                "",
                "[lbl1]: https://url1.com",
                "[lbl2]: https://url2.com",
            ],
            "doc1.md",
        );

        let folder = common::FakeFolder::mk(vec![doc1.clone()]);
        (doc1, folder)
    }

    #[test]
    fn on_ref_label() {
        let pos = Position::new(2, 7);
        let (doc1, folder) = mk_workspace();
        let res = refactor::rename(true, &folder, &doc1, pos, "newLbl");

        let expected_ranges: BTreeMap<String, Vec<(Range, String)>> = BTreeMap::from([(
            "doc1.md".to_string(),
            vec![
                (misc::range(5, 1, 5, 5), "newLbl".to_string()),
                (misc::range(2, 6, 2, 10), "newLbl".to_string()),
                (misc::range(1, 7, 1, 11), "newLbl".to_string()),
            ],
        )]);

        let actual_ranges = edits_by_file(&res);

        assert_eq!(
            expected_ranges.get("doc1.md").unwrap(),
            actual_ranges.get("doc1.md").unwrap()
        );
    }

    #[test]
    fn on_def_label() {
        let pos = Position::new(5, 3);
        let (doc1, folder) = mk_workspace();
        let res = refactor::rename(true, &folder, &doc1, pos, "newLbl");

        let expected_ranges: BTreeMap<String, Vec<(Range, String)>> = BTreeMap::from([(
            "doc1.md".to_string(),
            vec![
                (misc::range(5, 1, 5, 5), "newLbl".to_string()),
                (misc::range(2, 6, 2, 10), "newLbl".to_string()),
                (misc::range(1, 7, 1, 11), "newLbl".to_string()),
            ],
        )]);

        let actual_ranges = edits_by_file(&res);

        assert_eq!(
            expected_ranges.get("doc1.md").unwrap(),
            actual_ranges.get("doc1.md").unwrap()
        );
    }
}

// module RenameTests
//   module HeadingLinks

mod heading_links {
    use super::*;

    pub(super) fn mk_workspace() -> (
        marksman::doc::Doc,
        marksman::doc::Doc,
        marksman::folder::Folder,
    ) {
        let doc1 = fake_doc(
            //   0         1         2
            //   0123456789012345678901234567890
            &[
                "# Doc 1",
                "## Doc 1.2",
                "## Doc 1.3",
                "See [[#doc-12]]",
                "Also [](#doc-12)",
            ],
            "doc1.md",
        );

        let doc2 = fake_doc(
            //   0         1         2
            //   0123456789012345678901234567890
            &[
                "# Doc 2",
                "[[doc-1]]",
                "[[doc-1#doc-12]]",
                "[](/doc1.md#doc-12)",
                // filename wiki-link
                "[[doc1]]",
            ],
            "doc2.md",
        );

        let folder = common::FakeFolder::mk(vec![doc1.clone(), doc2.clone()]);
        (doc1, doc2, folder)
    }

    #[test]
    fn on_title() {
        let pos = Position::new(0, 3);
        let (doc1, _doc2, folder) = mk_workspace();
        let res = refactor::rename(true, &folder, &doc1, pos, "New Title");

        let expected_ranges: BTreeMap<String, Vec<(Range, String)>> = BTreeMap::from([
            (
                "doc1.md".to_string(),
                vec![(misc::range(0, 2, 0, 7), "New Title".to_string())],
            ),
            (
                "doc2.md".to_string(),
                vec![
                    (misc::range(2, 2, 2, 7), "new-title".to_string()),
                    (misc::range(1, 2, 1, 7), "new-title".to_string()),
                ],
            ),
        ]);

        let actual_ranges = edits_by_file(&res);

        assert_eq!(
            expected_ranges.get("doc1.md").unwrap(),
            actual_ranges.get("doc1.md").unwrap()
        );

        assert_eq!(
            expected_ranges.get("doc2.md").unwrap(),
            actual_ranges.get("doc2.md").unwrap()
        );
    }

    #[test]
    fn on_subtitle() {
        let pos = Position::new(1, 5);
        let (doc1, _doc2, folder) = mk_workspace();
        let res = refactor::rename(true, &folder, &doc1, pos, "New Title");

        let expected_ranges: BTreeMap<String, Vec<(Range, String)>> = BTreeMap::from([
            (
                "doc1.md".to_string(),
                vec![
                    (misc::range(4, 9, 4, 15), "new-title".to_string()),
                    (misc::range(3, 7, 3, 13), "New Title".to_string()),
                    (misc::range(1, 3, 1, 10), "New Title".to_string()),
                ],
            ),
            (
                "doc2.md".to_string(),
                vec![
                    (misc::range(3, 12, 3, 18), "new-title".to_string()),
                    (misc::range(2, 8, 2, 14), "New Title".to_string()),
                ],
            ),
        ]);

        let actual_ranges = edits_by_file(&res);

        assert_eq!(
            expected_ranges.get("doc1.md").unwrap(),
            actual_ranges.get("doc1.md").unwrap()
        );

        assert_eq!(
            expected_ranges.get("doc2.md").unwrap(),
            actual_ranges.get("doc2.md").unwrap()
        );
    }
}
