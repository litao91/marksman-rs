//! Port of `Tests/ServerTests.fs`.
//!
//! The original exercises `ServerUtil` (`calcTextSync`) and the diagnostics
//! publication calculation (`calcDiagnosticsUpdate`). Both are public in the
//! Rust port as `marksman::server::{calc_text_sync, calc_diagnostics_update}`.

mod common;

use std::sync::Arc;
use lsp_types::{Diagnostic, DiagnosticSeverity, PublishDiagnosticsParams, Range};
use marksman::config::{Config, ParserSettings, TextSync};
use marksman::diag;
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::server::{calc_diagnostics_update, calc_text_sync};
use marksman::state::{ClientDescription, State};
use marksman::workspace::Workspace;

use common::{dummy_root_path, mk_folder_id, FakeDoc, FakeFolder};

// module ServerUtilTests

#[test]
fn text_sync_user_config_empty_ws() {
    let c = Config { core_text_sync: Some(TextSync::Incremental), ..Config::default_config() };
    let client_desc = ClientDescription::empty();
    let ws = Workspace::of_folders(Some(c.clone()), Vec::<Folder>::new());
    assert_eq!(
        ("userConfig", TextSync::Incremental),
        calc_text_sync(&Some(c), &ws, &client_desc)
    );
}

#[test]
fn text_sync_no_config_empty_ws() {
    let client_desc = ClientDescription::empty();
    let ws = Workspace::of_folders(None, Vec::<Folder>::new());
    assert_eq!(("default", TextSync::Full), calc_text_sync(&None, &ws, &client_desc));
}

#[test]
fn text_sync_no_config_empty_ws_prefer_incr() {
    let client_desc = ClientDescription {
        opts: marksman::state::InitOptions {
            preferred_text_sync_kind: Some(TextSync::Incremental),
        },
        ..ClientDescription::empty()
    };

    let ws = Workspace::of_folders(None, Vec::<Folder>::new());
    assert_eq!(
        ("clientOption", TextSync::Incremental),
        calc_text_sync(&None, &ws, &client_desc)
    );
}

#[test]
fn text_sync_no_config_non_empty_ws_prefer_incr() {
    let client_desc = ClientDescription {
        opts: marksman::state::InitOptions {
            preferred_text_sync_kind: Some(TextSync::Incremental),
        },
        ..ClientDescription::empty()
    };

    let folder = Folder::multi_file(
        "test".to_string(),
        mk_folder_id(&dummy_root_path(&["test"])),
        Vec::new(),
        None,
    );

    let ws = Workspace::of_folders(None, vec![folder]);
    assert_eq!(
        ("clientOption", TextSync::Incremental),
        calc_text_sync(&None, &ws, &client_desc)
    );
}

#[test]
fn text_sync_non_empty_ws_prefer_incr_but_config_takes_precedence() {
    let client_desc = ClientDescription {
        opts: marksman::state::InitOptions {
            preferred_text_sync_kind: Some(TextSync::Incremental),
        },
        ..ClientDescription::empty()
    };

    let folder = Folder::multi_file(
        "test".to_string(),
        mk_folder_id(&dummy_root_path(&["test"])),
        Vec::new(),
        Some(Config { core_text_sync: Some(TextSync::Full), ..Config::empty() }),
    );

    let ws = Workspace::of_folders(None, vec![folder]);
    assert_eq!(("workspaceConfig", TextSync::Full), calc_text_sync(&None, &ws, &client_desc));
}

// module DiagnosticPublicationTests

/// `let private doc path lines = FakeDoc.Mk(path = path, contentLines = Array.ofList lines)`
fn doc(path: &str, lines: &[&str]) -> Doc {
    FakeDoc::mk_with(&lines.join("\n"), path, None, None)
}

/// `let private state folder = Workspace.ofFolders None [ folder ] |> State.mk ClientDescription.empty`
fn state(folder: Folder) -> State {
    State::mk(ClientDescription::empty(), Workspace::of_folders(None, vec![folder]))
}

/// `let private publications previous current = ...`
fn publications(
    previous: Option<&State>,
    current: &State,
) -> Vec<PublishDiagnosticsParams> {
    let previous_diag = previous.map(|state| diag::calculate(None, state.workspace()).0);
    let previous = previous.zip(previous_diag.as_ref());
    calc_diagnostics_update(previous, current).1
}

/// `let private onlyPublication (doc: Doc) (updates: PublishDiagnosticsParams[])`
fn only_publication(doc: &Doc, updates: &[PublishDiagnosticsParams]) -> Vec<Diagnostic> {
    assert_eq!(1, updates.len(), "expected a single publication, got {updates:#?}");
    let update = &updates[0];
    assert_eq!(doc.uri(), update.uri.as_str());
    assert_eq!(None, update.version);
    update.diagnostics.clone()
}

/// `Assert.Single`
fn single<T>(items: Vec<T>) -> T {
    assert_eq!(1, items.len(), "expected a single item, got {}", items.len());
    items.into_iter().next().unwrap()
}

#[test]
fn initial_calculation_publishes_only_documents_with_diagnostics() {
    let source = doc("source.md", &["[[missing]]", "##\u{a0}Not a heading"]);
    let clean = doc("clean.md", &["Some prose."]);

    let current = state(FakeFolder::mk(vec![source.clone(), clean]));
    let diagnostics = publications(None, &current);

    let source_diagnostics = only_publication(&source, &diagnostics);

    assert_eq!(2, source_diagnostics.len());
    assert_eq!("Link to non-existent document 'missing'", source_diagnostics[0].message);

    assert!(
        source_diagnostics[1]
            .message
            .starts_with("Non-breaking whitespace used instead of regular whitespace"),
        "unexpected message: {}",
        source_diagnostics[1].message
    );

    assert_eq!(Some(DiagnosticSeverity::ERROR), source_diagnostics[0].severity);
    assert_eq!(Some(DiagnosticSeverity::WARNING), source_diagnostics[1].severity);
    assert_eq!(0, source_diagnostics[0].range.start.line);
    assert_eq!(1, source_diagnostics[1].range.start.line);
}

#[test]
fn unchanged_diagnostics_produce_no_publication() {
    let source = doc("source.md", &["[[missing]]"]);
    let before = state(FakeFolder::mk(vec![source]));
    let edited = doc("source.md", &["[[missing]]", "More prose."]);
    let after = state(FakeFolder::mk(vec![edited]));

    assert!(publications(Some(&before), &after).is_empty());
}

#[test]
fn unrelated_edit_reuses_cached_diagnostics() {
    let source = doc("source.md", &["[[missing]]"]);
    let unrelated = doc("unrelated.md", &["Some prose."]);
    let before_folder = FakeFolder::mk(vec![source.clone(), unrelated]);
    let before = state(before_folder.clone());
    let edited = doc("unrelated.md", &["More prose."]);
    let after = state(before_folder.with_doc(edited));
    let (before_diag, _) = calc_diagnostics_update(None, &before);

    let (after_diag, updates) = calc_diagnostics_update(Some((&before, &before_diag)), &after);

    let folder_id = before_folder.id();
    let source_before = &before_diag[&folder_id][source.id()];
    let source_after = &after_diag[&folder_id][source.id()];

    // F#: `Assert.True(obj.ReferenceEquals(sourceBefore, sourceAfter))`. The
    // cached diagnostics are `Arc`-shared, so an unaffected document keeps the
    // very same vector rather than a copy.
    assert!(Arc::ptr_eq(source_before, source_after));
    assert!(updates.is_empty());
}

#[test]
fn successive_updates_use_the_last_calculated_snapshot() {
    let source = doc("source.md", &["[[Target#Section]]"]);
    let first = doc("first.md", &["# Target", "## Section"]);
    let second = doc("second.md", &["# Target", "## Section"]);
    let initial_folder = FakeFolder::mk(vec![source.clone()]);
    let initial = state(initial_folder.clone());
    let (initial_diag, _) = calc_diagnostics_update(None, &initial);

    let resolved_folder = initial_folder.with_doc(first);
    let resolved = state(resolved_folder.clone());

    let (resolved_diag, cleared) =
        calc_diagnostics_update(Some((&initial, &initial_diag)), &resolved);

    assert!(only_publication(&source, &cleared).is_empty());

    let ambiguous = state(resolved_folder.with_doc(second));

    let (_, reported) = calc_diagnostics_update(Some((&resolved, &resolved_diag)), &ambiguous);

    let diagnostic = single(only_publication(&source, &reported));

    assert_eq!("Ambiguous link to heading 'section' in document 'Target'", diagnostic.message);
}

#[test]
fn other_documents_can_resolve_or_make_a_link_ambiguous() {
    let source = doc("source.md", &["[[Target#Section]]"]);
    let first = doc("first.md", &["# Target", "## Section"]);
    let second = doc("second.md", &["# Target", "## Section"]);
    let missing = FakeFolder::mk(vec![source.clone()]);
    let resolved = missing.with_doc(first);
    let ambiguous = resolved.with_doc(second);

    let missing_state = state(missing);
    let broken = only_publication(&source, &publications(None, &missing_state));
    let broken_link = single(broken);

    assert_eq!(
        "Link to non-existent heading 'section' in document 'Target'",
        broken_link.message
    );

    let resolved_state = state(resolved);
    let cleared = only_publication(
        &source,
        &publications(Some(&missing_state), &resolved_state),
    );

    assert!(cleared.is_empty());

    let ambiguous_state = state(ambiguous);
    let reported = only_publication(
        &source,
        &publications(Some(&resolved_state), &ambiguous_state),
    );

    let diagnostic = single(reported);
    assert_eq!("Ambiguous link to heading 'section' in document 'Target'", diagnostic.message);
    assert_eq!(2, diagnostic.related_information.unwrap().len());
}

#[test]
fn moving_a_target_heading_updates_ambiguous_link_related_location() {
    let source = doc("source.md", &["[[Target#Section]]"]);
    let first = doc("first.md", &["# Target", "## Section"]);
    let second = doc("second.md", &["# Target", "## Section"]);
    let before = FakeFolder::mk(vec![source.clone(), first.clone(), second]);
    let moved = doc("first.md", &["# Target", "", "## Section"]);
    let after = before.with_doc(moved);

    let before_state = state(before);
    let after_state = state(after);

    let prior = only_publication(&source, &publications(None, &before_state));

    let latest =
        only_publication(&source, &publications(Some(&before_state), &after_state));

    let prior_diagnostic = single(prior);
    let latest_diagnostic = single(latest);

    let first_uri = first.uri().to_string();
    let range_in_first = |diagnostic: &Diagnostic| -> Range {
        diagnostic
            .related_information
            .as_ref()
            .unwrap()
            .iter()
            .find(|related| related.location.uri.as_str() == first_uri)
            .expect("a related location in first.md")
            .location
            .range
    };

    assert_eq!(1, range_in_first(&prior_diagnostic).start.line);
    assert_eq!(2, range_in_first(&latest_diagnostic).start.line);

    assert_eq!(
        "Ambiguous link to heading 'section' in document 'Target'",
        latest_diagnostic.message
    );
}

#[test]
fn removing_one_of_two_targets_clears_ambiguity() {
    let source = doc("source.md", &["[[Target#Section]]"]);
    let first = doc("first.md", &["# Target", "## Section"]);
    let second = doc("second.md", &["# Target", "## Section"]);
    let before = FakeFolder::mk(vec![source.clone(), first, second.clone()]);
    let after = before.without_doc(second.id()).unwrap();

    let cleared = only_publication(
        &source,
        &publications(Some(&state(before)), &state(after)),
    );

    assert!(cleared.is_empty());
}

#[test]
fn removing_a_document_clears_its_published_diagnostics() {
    let source = doc("source.md", &["[[missing]]"]);
    let remaining = doc("remaining.md", &["Some prose."]);
    let before = FakeFolder::mk(vec![source.clone(), remaining]);
    let after = before.without_doc(source.id()).unwrap();

    let cleared = only_publication(
        &source,
        &publications(Some(&state(before)), &state(after)),
    );

    assert!(cleared.is_empty());
}

#[test]
fn removing_a_folder_clears_diagnostics_for_its_documents() {
    let source = doc("source.md", &["[[missing]]"]);
    let before = state(FakeFolder::mk(vec![source.clone()]));

    let after = State::mk(
        ClientDescription::empty(),
        Workspace::of_folders(None, Vec::<Folder>::new()),
    );

    let cleared = only_publication(&source, &publications(Some(&before), &after));

    assert!(cleared.is_empty());
}

#[test]
fn reopening_a_document_resends_unchanged_diagnostics() {
    let source = doc("source.md", &["[[missing]]"]);
    let before = FakeFolder::mk(vec![source.clone()]);

    let reopened = Doc::mk(
        &ParserSettings::default(),
        source.id().clone(),
        Some(1),
        source.text().clone(),
    )
    .unwrap();

    let after = before.with_doc(reopened);

    let reported = only_publication(
        &source,
        &publications(Some(&state(before)), &state(after)),
    );

    let broken_link = single(reported);
    assert_eq!("Link to non-existent document 'missing'", broken_link.message);
}

#[test]
fn reopening_a_clean_document_does_not_publish_an_empty_update() {
    let source = doc("source.md", &["Some prose."]);
    let before = FakeFolder::mk(vec![source.clone()]);

    let reopened = Doc::mk(
        &ParserSettings::default(),
        source.id().clone(),
        Some(1),
        source.text().clone(),
    )
    .unwrap();

    let after = before.with_doc(reopened);

    assert!(publications(Some(&state(before)), &state(after)).is_empty());
}

#[test]
fn publication_compares_last_published_state_with_latest_debounced_state() {
    let source = doc("source.md", &["[[Target]]"]);
    let target = doc("target.md", &["# Target"]);
    let initial = FakeFolder::mk(vec![source]);
    let intermediate = initial.with_doc(target.clone());
    let latest = intermediate.without_doc(target.id()).unwrap();

    assert!(publications(Some(&state(initial)), &state(latest)).is_empty());
}
