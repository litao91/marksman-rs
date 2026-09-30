//! Port of `Tests/WorkspaceTest.fs`.

mod common;

use std::collections::BTreeSet;

use lsp_types::{TextDocumentContentChangeEvent, TextDocumentItem};
use marksman::config::{Config, ParserSettings};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::misc;
use marksman::misc::Slug;
use marksman::names::{DocId, InternName, InternPath};
use marksman::paths::{AbsPath, RelPath, UriWith};
use marksman::text::mk_text;
use marksman::workspace::Workspace;

use common::{
    dummy_root, dummy_root_path, mk_doc_id, mk_folder_id, path_to_uri, FakeDoc, FakeFolder,
};

/// `Folder` has no `PartialEq` in the Rust port, so snapshots are compared by
/// the parts the original's structural equality looks at.
fn same_folder(a: Option<&Folder>, b: Option<&Folder>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => {
            x.id() == y.id() && x.docs() == y.docs() && x.config() == y.config()
        }
        _ => false,
    }
}

fn doc_ids(docs: &[&Doc]) -> BTreeSet<DocId> {
    docs.iter().map(|d| d.id().clone()).collect()
}

/// `Approx(RelPath "...")`
fn approx(path: &str) -> InternPath {
    InternPath::Approx(RelPath::of_string_unchecked(path))
}

fn rel(path: &str) -> RelPath {
    RelPath::of_string_unchecked(path)
}

// module FolderTest

#[test]
fn docs_difference_across_interleaved_paths() {
    let a = FakeDoc::mk_with("A", "a.md", None, None);
    let c = FakeDoc::mk_with("C", "c.md", None, None);
    let changed_c = FakeDoc::mk_with("Updated C", "c.md", None, None);
    let e = FakeDoc::mk_with("E", "e.md", None, None);
    let g = FakeDoc::mk_with("G", "g.md", None, None);
    let b = FakeDoc::mk_with("B", "b.md", None, None);
    let d = FakeDoc::mk_with("D", "d.md", None, None);
    let z = FakeDoc::mk_with("Z", "z.md", None, None);
    let before = FakeFolder::mk(vec![a.clone(), c.clone(), e.clone(), g.clone()]);
    let after = FakeFolder::mk(vec![
        a.clone(),
        b.clone(),
        changed_c.clone(),
        d.clone(),
        g.clone(),
        z.clone(),
    ]);
    let difference = Folder::docs_difference(&before, &after);

    assert_eq!(doc_ids(&[&b, &d, &z]), difference.added);
    assert_eq!(doc_ids(&[&e]), difference.removed);
    assert_eq!(doc_ids(&[&c]), difference.changed);
    assert!(difference.reopened.is_empty());
}

#[test]
fn docs_difference_reports_reopening_without_a_content_change() {
    let closed = FakeDoc::mk_with("A", "a.md", None, None);

    let reopened = Doc::mk(
        &ParserSettings::default(),
        closed.id().clone(),
        Some(1),
        closed.text().clone(),
    )
    .unwrap();

    let before = FakeFolder::mk(vec![closed.clone()]);
    let after = before.with_doc(reopened);
    let difference = Folder::docs_difference(&before, &after);

    assert_eq!(doc_ids(&[&closed]), difference.reopened);
    assert!(difference.changed.is_empty());
}

#[test]
fn roo_path_single_file() {
    let d1 = FakeDoc::mk_with("", "a/b/d1.md", Some(&["a"]), None);
    let f1 = Folder::single_file(d1, None);

    assert_eq!(mk_folder_id(&dummy_root_path(&["a"])).data, f1.root_path());
}

#[test]
fn update_doc() {
    let d1 = FakeDoc::mk_with("# Title 1", "doc1.md", None, None);
    let d2 = FakeDoc::mk_with("# Title 2", "doc2.md", None, None);
    let f = FakeFolder::mk(vec![d1.clone(), d2.clone()]);

    assert_eq!(1, f.filter_docs_by_slug(&Slug::of_string("Title 1")).len());
    assert_eq!(0, f.filter_docs_by_slug(&Slug::of_string("Title 0")).len());

    assert_eq!(1, f.filter_docs_by_intern_path(&approx("doc1")).len());
    assert_eq!(0, f.filter_docs_by_intern_path(&approx("doc0")).len());

    // Updating a title of the doc; by slug lookup should reflect
    let d1 = FakeDoc::mk_with("# Title 2", "doc1.md", None, None);
    let f = f.with_doc(d1.clone());

    assert_eq!(0, f.filter_docs_by_slug(&Slug::of_string("Title 1")).len());
    assert_eq!(2, f.filter_docs_by_slug(&Slug::of_string("Title 2")).len());

    assert_eq!(1, f.filter_docs_by_intern_path(&approx("doc1")).len());
    assert_eq!(1, f.filter_docs_by_intern_path(&approx("doc2")).len());

    // Removing a doc; both by slug and by path should reflect
    let f = f.without_doc(d1.id()).unwrap();

    assert_eq!(0, f.filter_docs_by_slug(&Slug::of_string("Title 1")).len());
    assert_eq!(1, f.filter_docs_by_slug(&Slug::of_string("Title 2")).len());

    assert_eq!(0, f.filter_docs_by_intern_path(&approx("doc1")).len());
    assert_eq!(1, f.filter_docs_by_intern_path(&approx("doc2")).len());

    // Adding a new doc; both by slug and by path should reflect
    let d3 = FakeDoc::mk_with("# Title 3", "doc3.md", None, None);
    let f = f.with_doc(d3);
    assert_eq!(1, f.filter_docs_by_intern_path(&approx("doc3")).len());
    assert_eq!(0, f.filter_docs_by_intern_path(&approx("doc4")).len());

    assert_eq!(1, f.filter_docs_by_slug(&Slug::of_string("Title 3")).len());
    assert_eq!(0, f.filter_docs_by_slug(&Slug::of_string("Title 4")).len());
}

#[test]
fn colliding_canonical_paths_keep_both_documents() {
    let markdown = FakeDoc::mk_with("# Markdown", "notes.md", None, None);

    let long_extension = FakeDoc::mk_with("# Long extension", "notes.markdown", None, None);

    let config = Config {
        core_incremental_references: Some(true),
        core_paranoid: Some(true),
        ..Config::default_config()
    };

    let folder = FakeFolder::mk_with_config(vec![markdown.clone()], Some(config))
        .with_doc(long_extension.clone());

    let markdown_path = markdown.path();
    let long_extension_path = long_extension.path();

    assert_eq!(2, folder.doc_count());
    assert_eq!(Some(markdown.clone()), folder.try_find_doc_by_path(&markdown_path));
    assert_eq!(
        Some(long_extension.clone()),
        folder.try_find_doc_by_path(&long_extension_path)
    );
    assert_eq!(None, folder.try_find_doc_by_rel_path(&rel("notes")));
    assert_eq!(Some(markdown.clone()), folder.try_find_doc_by_rel_path(&rel("notes.md")));

    assert_eq!(2, folder.filter_docs_by_intern_path(&approx("notes")).len());

    assert_eq!(
        2,
        folder
            .filter_docs_by_name(&InternName::mk_unchecked(markdown.id().clone(), "notes"))
            .len()
    );

    let without_markdown = folder.without_doc(markdown.id()).unwrap();
    assert_eq!(1, without_markdown.doc_count());

    assert_eq!(
        Some(long_extension.clone()),
        without_markdown.try_find_doc_by_path(&long_extension_path)
    );

    assert_eq!(1, without_markdown.filter_docs_by_intern_path(&approx("notes")).len());

    let updated =
        folder.with_doc(FakeDoc::mk_with("# Updated", "notes.markdown", None, None));

    assert_eq!(2, updated.doc_count());
    assert_eq!(Some(markdown), updated.try_find_doc_by_path(&markdown_path));
}

#[test]
fn extension_change_keeps_documents_that_now_share_a_canonical_path() {
    let markdown = FakeDoc::mk_with("# Markdown", "notes.md", None, None);

    let long_extension = FakeDoc::mk_with("# Long extension", "notes.markdown", None, None);

    let initial_config = Config {
        core_markdown_file_extensions: Some(vec!["md".to_string()]),
        ..Config::default_config()
    };

    let folder = FakeFolder::mk_with_config(
        vec![markdown.clone(), long_extension.clone()],
        Some(initial_config.clone()),
    );

    let updated_config = Config {
        core_markdown_file_extensions: Some(vec!["md".to_string(), "markdown".to_string()]),
        ..initial_config
    };

    let updated = folder.with_config(Some(updated_config));

    assert_eq!(2, updated.doc_count());

    assert_eq!(2, updated.filter_docs_by_intern_path(&approx("notes")).len());

    let markdown_path = markdown.path();
    let long_extension_path = long_extension.path();

    assert_eq!(Some(markdown), updated.try_find_doc_by_path(&markdown_path));
    assert_eq!(
        Some(long_extension),
        updated.try_find_doc_by_path(&long_extension_path)
    );
}

// module DocTest

#[test]
fn apply_lsp_change() {
    let dummy_path = mk_doc_id(&mk_folder_id(dummy_root()), &dummy_root_path(&["dummy.md"]));

    let empty =
        Doc::mk(&ParserSettings::default(), dummy_path.clone(), None, mk_text("")).unwrap();

    let insert_change = TextDocumentContentChangeEvent {
        range: Some(misc::range(0, 0, 0, 0)),
        range_length: Some(0),
        text: "[".to_string(),
    };
    // The original wraps the change in a `TextDocumentContentChangeParams`
    // carrying `{ Uri = RootedRelPath.toSystem dummyPath.Path; Version = 1 }`;
    // the Rust `apply_lsp_change` takes the change list and the version directly.
    let updated =
        Doc::apply_lsp_change(&ParserSettings::default(), &[insert_change], 1, &empty).unwrap();

    assert_eq!("[", updated.text().content);
}

// The original carries `[<Fact(Skip = "Uri and # don't mix well")>]`: F#'s
// Skipped upstream as `Fact(Skip = "Uri and # don't mix well")`: `System.Uri`
// drops everything after a `#`, so the document loses its `.md` name. The port
// reproduces that behaviour, and the failure with it.
#[ignore = "Uri and # don't mix well (skipped in the original too)"]
#[test]
fn path_from_root_special_chars() {
    let doc = FakeDoc::mk_with("", "blah#blah.md", None, None);
    assert_eq!("blah#blah.md", doc.path_from_root().to_system());
}

#[test]
fn from_lsp_single_file() {
    let par = TextDocumentItem {
        uri: marksman::uri::parse("file:///a/b/doc.md"),
        language_id: "md".to_string(),
        version: 1,
        text: "text".to_string(),
    };

    let singleton_root = UriWith::mk_root(par.uri.to_string());
    let doc = Doc::from_lsp(&ParserSettings::default(), &singleton_root, &par).unwrap();
    assert_eq!("file:///a/b/doc.md", doc.uri());
    // The original asserts `(Doc.path doc).ToString()` equals `AbsPath "/a/b/doc.md"`,
    // i.e. F#'s rendering of the `AbsPath` single-case union. The Rust `AbsPath`
    // is a tuple struct, so the wrapped path is compared instead.
    assert_eq!(AbsPath("/a/b/doc.md".to_string()), doc.path());
}

// module WorkspaceTest

#[test]
fn folder_find_single_file() {
    let f1 = {
        let d = FakeDoc::mk_with("", "a/b/d1.md", Some(&["a"]), None);
        Folder::single_file(d, None)
    };

    let f2 = {
        let d = FakeDoc::mk_with("", "a/b/d2.md", Some(&["a"]), None);
        Folder::single_file(d, None)
    };

    let ws = Workspace::of_folders(None, vec![f1.clone(), f2.clone()]);

    assert!(same_folder(
        Some(&f1),
        ws.try_find_folder_enclosing(&AbsPath::of_uri(&path_to_uri(&dummy_root_path(&[
            "a", "b", "d1.md"
        ]))))
    ));

    assert!(same_folder(
        None,
        ws.try_find_folder_enclosing(&AbsPath::of_uri(&path_to_uri(&dummy_root_path(&[
            "a", "d1.md"
        ]))))
    ));
}

#[test]
fn folder_added_evict_single_file() {
    let d1 = FakeDoc::mk_with("", "a/b/d1.md", Some(&["a", "b"]), None);
    let f1 = Folder::single_file(d1, None);
    let d2 = FakeDoc::mk_with("", "c/d2.md", Some(&["c"]), None);
    let f2 = Folder::single_file(d2, None);

    let ws = Workspace::of_folders(None, vec![f1, f2.clone()]);

    let f0_path = mk_folder_id(&dummy_root_path(&["a"]));
    let f0 = Folder::multi_file("f0".to_string(), f0_path, Vec::new(), None);

    let upd_ws = ws.with_folder(f0.clone());

    let ids = upd_ws.folders().map(Folder::id).collect::<Vec<_>>();

    assert_eq!(vec![f0.id(), f2.id()], ids);
}

#[test]
fn folder_config_no_user_config() {
    let f_config = Some(Config { ca_toc_enable: Some(false), ..Config::default_config() });
    let f_path = mk_folder_id(&dummy_root_path(&["a"]));

    // Multi-file
    let f =
        Folder::multi_file("f0".to_string(), f_path.clone(), Vec::new(), f_config.clone());
    let ws = Workspace::of_folders(None, vec![f]);
    let f = ws.folders().next().unwrap();
    let updated_config = f.config();

    assert_eq!(f_config, updated_config);

    // Single-file
    let f = Folder::single_file(FakeDoc::mk(""), f_config.clone());
    let ws = Workspace::of_folders(None, vec![f]);
    let f = ws.folders().next().unwrap();
    let updated_config = f.config();
    assert_eq!(f_config, updated_config);
}

#[test]
fn folder_config_user_config() {
    let ws_config = Some(Config { ca_toc_enable: Some(false), ..Config::default_config() });
    let f_path = mk_folder_id(&dummy_root_path(&["a"]));

    // Multi-file
    let f = Folder::multi_file("f0".to_string(), f_path, Vec::new(), None);
    let ws = Workspace::of_folders(ws_config.clone(), vec![f]);
    let f = ws.folders().next().unwrap();
    let updated_config = f.config();

    assert_eq!(ws_config, updated_config);

    // Single-file
    let f = Folder::single_file(FakeDoc::mk(""), None);
    let ws = Workspace::of_folders(ws_config.clone(), vec![f]);
    let f = ws.folders().next().unwrap();
    let updated_config = f.config();
    assert_eq!(ws_config, updated_config);
}

#[test]
fn folder_config_user_config_folder_add() {
    let ws_config = Some(Config { ca_toc_enable: Some(false), ..Config::default_config() });
    let f_path = mk_folder_id(&dummy_root_path(&["a"]));

    // Multi-file
    let ws = Workspace::of_folders(ws_config.clone(), Vec::<Folder>::new());
    let f = Folder::multi_file("f0".to_string(), f_path, Vec::new(), None);
    let ws = ws.with_folder(f);
    let f = ws.folders().next().unwrap();
    let updated_config = f.config();

    assert_eq!(ws_config, updated_config);

    // Single-file
    let ws = Workspace::of_folders(ws_config.clone(), Vec::<Folder>::new());
    let f = Folder::single_file(FakeDoc::mk(""), None);
    let ws = ws.with_folder(f);
    let f = ws.folders().next().unwrap();
    let updated_config = f.config();
    assert_eq!(ws_config, updated_config);
}
