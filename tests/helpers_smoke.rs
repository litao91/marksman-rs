mod common;

use common::{FakeDoc, FakeFolder, TempWorkspace, apply_document_action, strip_margin_trim};
use marksman::code_actions;
use marksman::config::Config;

#[test]
fn fake_doc_has_the_dummy_root_and_path() {
    let doc = FakeDoc::mk("# Title\n");
    assert_eq!(doc.uri(), "file:///fake.md");
    assert_eq!(doc.name(), "Title");
    assert_eq!(doc.path_from_root().to_system(), "fake.md");
}

#[test]
fn fake_doc_at_a_nested_path() {
    let doc = FakeDoc::mk_at("body\n", "dir/sub/note.md");
    assert_eq!(doc.uri(), "file:///dir/sub/note.md");
    assert_eq!(doc.path_from_root().to_system(), "dir/sub/note.md");
    assert_eq!(doc.name(), "note");
}

#[test]
fn fake_folder_indexes_the_documents_it_is_given() {
    let folder = FakeFolder::mk(vec![FakeDoc::mk_at("# A\n", "a.md"), FakeDoc::mk_at("# B\n", "b.md")]);
    assert_eq!(folder.doc_count(), 2);
    assert_eq!(folder.id().uri, "file:///");
}

#[test]
fn strip_margin_matches_the_fsharp_helper() {
    // The original strips `^[\s]*|` from every line, so a line that is only a
    // pipe becomes an empty line.
    assert_eq!(strip_margin_trim("|a\n|  b\n|---\n"), "a\n  b\n---");
    assert_eq!(strip_margin_trim("|\n|a\n"), "\na");
}

#[test]
fn document_action_is_applied_to_the_text() {
    let doc = FakeDoc::mk("# T1\n## T2\n");
    let action = code_actions::table_of_contents(&Config::default_config(), &doc).unwrap();
    let updated = apply_document_action(&doc, &action);
    assert!(updated.contains("<!--toc:start-->"));
    assert!(updated.contains("- [T1](#t1)"));
    assert!(updated.starts_with("# T1\n"));
}

#[test]
fn temp_workspace_loads_documents_from_disk() {
    let ws = TempWorkspace::new("helpers-smoke").with_marker();
    ws.write("a.md", "# A\n");
    ws.write("sub/b.md", "# B\n");
    let folder = ws.load();
    assert_eq!(folder.doc_count(), 2);
    assert_eq!(ws.doc_named(&folder, "B").path_from_root().to_system(), "sub/b.md");
}
