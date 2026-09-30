//! Port of `Tests/CodeActionTests.fs`.

mod common;

use common::FakeFolder;
use marksman::code_actions;
use marksman::misc;

/// `FakeDoc.Mk(contentLines, path = ...)`.
fn fake_doc(lines: &[&str], path: &str) -> marksman::doc::Doc {
    common::FakeDoc::mk_with(&lines.join("\n"), path, None, None)
}

// module CreateMissingFileTests
//
// The original threads a `CodeActionContext` through `createMissingFile`; the
// Rust port's `create_missing_file` takes only the range, the document and the
// folder, so the (always empty) context is dropped here.

#[test]
fn should_create_when_no_file_exists() {
    let doc1 = fake_doc(&["# Doc 1", "## Sub 1"], "doc1.md");
    let doc2 = fake_doc(&["[[doc3]]"], "doc2.md");
    let folder = FakeFolder::mk(vec![doc1.clone(), doc2.clone()]);

    let ca = code_actions::create_missing_file(misc::range(0, 3, 0, 3), &doc2, &folder);

    match ca {
        Some(action) if action.name == "Create `doc3.md`" => assert!(true),
        other => panic!("expected a `Create `doc3.md`` action, got {other:?}"),
    }
}

#[test]
fn should_not_create_when_ref_broken_but_file_exists() {
    let doc1 = fake_doc(&["# Doc 1", "## Sub 1"], "doc1.md");
    let doc2 = fake_doc(&["[[doc1#Sub 2]]"], "doc2.md");
    let folder = FakeFolder::mk(vec![doc1.clone(), doc2.clone()]);

    let ca = code_actions::create_missing_file(misc::range(0, 3, 0, 3), &doc2, &folder);

    assert!(ca.is_none(), "expected no action, got {ca:?}");
}

#[test]
fn should_not_create_when_titleless_file_exists() {
    let doc1 = fake_doc(&["Body without a title."], "doc1.md");
    let doc2 = fake_doc(&["[[doc1]]"], "doc2.md");
    let folder = FakeFolder::mk(vec![doc1.clone(), doc2.clone()]);

    let ca = code_actions::create_missing_file(misc::range(0, 3, 0, 3), &doc2, &folder);

    assert!(ca.is_none(), "expected no action, got {ca:?}");
}
