//! Port of `Tests/StateTests.fs`.

mod common;

use marksman::config::TextSync;
use marksman::folder::Folder;
use marksman::paths::UriWith;
use marksman::state::{ClientDescription, InitOptions, State};
use marksman::workspace::Workspace;
use serde_json::Value;

use common::{dummy_root_path, path_to_uri, FakeDoc};

/// `InitOptions` has no `PartialEq` in the Rust port; it has exactly one field,
/// so comparing that field is the whole-struct comparison the original does.
fn assert_init_options(expected: InitOptions, actual: InitOptions) {
    assert_eq!(expected.preferred_text_sync_kind, actual.preferred_text_sync_kind);
}

fn parse(json: &str) -> Value {
    serde_json::from_str(json).unwrap()
}

// module InitOptionTests

#[test]
fn extract_empty() {
    let json = parse("{}");
    assert_init_options(InitOptions::default(), InitOptions::of_json(&json));

    let json = parse("[]");
    assert_init_options(InitOptions::default(), InitOptions::of_json(&json));
}

#[test]
fn extract_correct() {
    let json = parse(r#"{"preferredTextSyncKind": 1}"#);
    assert_init_options(
        InitOptions { preferred_text_sync_kind: Some(TextSync::Full) },
        InitOptions::of_json(&json),
    );

    let json = parse(r#"{"preferredTextSyncKind": 2}"#);
    assert_init_options(
        InitOptions { preferred_text_sync_kind: Some(TextSync::Incremental) },
        InitOptions::of_json(&json),
    );
}

#[test]
fn extract_malformed() {
    let json = parse(r#"{"preferredTextSyncKind": 42}"#);
    assert_init_options(
        InitOptions { preferred_text_sync_kind: None },
        InitOptions::of_json(&json),
    );

    let json = parse(r#"{"preferredTextSyncKind": "full"}"#);
    assert_init_options(
        InitOptions { preferred_text_sync_kind: None },
        InitOptions::of_json(&json),
    );
}

// module StateTests

#[test]
fn folder_find_single_file() {
    let d1 = FakeDoc::mk_with("", "a/b/d1.md", Some(&["a"]), None);
    let f1 = Folder::single_file(d1.clone(), None);

    let d2 = FakeDoc::mk_with("", "a/b/d2.md", Some(&["a"]), None);
    let f2 = Folder::single_file(d2, None);

    let ws = Workspace::of_folders(None, vec![f1.clone(), f2]);
    let state = State::mk(ClientDescription::empty(), ws);

    let d1_uri = path_to_uri(&dummy_root_path(&["a", "b", "d1.md"]));
    let found = state.try_find_folder_and_doc(&UriWith::mk_abs(d1_uri));

    // `Assert.Equal(Some(f1, d1), ...)`: `Folder` has no `PartialEq`, so the
    // folder is compared by identity of its snapshot contents.
    let (folder, doc) = found.expect("Some(f1, d1)");
    assert_eq!(f1.id(), folder.id());
    assert_eq!(f1.docs(), folder.docs());
    assert_eq!(f1.config(), folder.config());
    assert_eq!(d1, doc);
}
