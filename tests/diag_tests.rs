//! Port of `Tests/DiagTest.fs`.

mod common;

use std::collections::BTreeSet;

use marksman::config::{Config, ParserSettings};
use marksman::conn::Conn;
use marksman::diag::{self, Entry};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::names::{DocId, FolderId, InternPath};
use marksman::paths::RelPath;
use marksman::workspace::Workspace;

use common::{dummy_root_path, mk_folder_id, FakeDoc, FakeFolder};

/// `diagToHuman`: the `(relative path, message)` pairs a folder publishes.
fn diag_to_human(folder: &Folder) -> Vec<(String, String)> {
    let (diagnostics, _) =
        diag::calculate(None, &Workspace::of_folders(None, vec![folder.clone()]));

    let mut out = Vec::new();
    for (id, entries) in &diagnostics[&folder.id()] {
        let path = id.path().rel_path_forced().to_system().to_string();
        for entry in entries.iter() {
            out.push((path.clone(), entry.message.clone()));
        }
    }
    out
}

fn expected_pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

/// `FakeDoc.Mk(path = ..., contentLines = [| ... |])`
fn doc_lines(path: &str, lines: &[&str]) -> Doc {
    FakeDoc::mk_at(&lines.join("\n"), path)
}

fn full_diagnostics(folder: &Folder) -> diag::FolderDiag {
    let (diags, _) = diag::calculate(None, &Workspace::of_folders(None, vec![folder.clone()]));
    diags.get(&folder.id()).cloned().unwrap_or_default()
}

#[test]
fn document_index_1() {
    let doc = FakeDoc::mk("# T1\n# T2");

    let titles: Vec<&str> = doc.index().titles.iter().map(|x| x.data.title.text.as_str()).collect();

    assert_eq!(vec!["T1", "T2"], titles);
}

#[test]
fn non_breaking_whitespace() {
    let nbsp = "\u{00a0}";
    let doc = FakeDoc::mk(&format!("# T1\n##{nbsp}T2"));

    match diag::check_non_breaking_whitespace(&doc).as_slice() {
        [Entry::NonBreakableWhitespace(range)] => {
            assert_eq!(1, range.start.line);
            assert_eq!(1, range.end.line);

            assert_eq!(2, range.start.character);
            assert_eq!(3, range.end.character);
        }
        _ => panic!("Expected NonBreakingWhitespace diagnostic"),
    }
}

#[test]
fn no_diag_on_shortcut_links() {
    let doc = FakeDoc::mk_lines(&["# H1", "## H2", "[shortcut]", "[[#h42]]"]);
    let folder = FakeFolder::mk(vec![doc]);
    let diag = diag_to_human(&folder);

    assert_eq!(
        expected_pairs(&[("fake.md", "Link to non-existent heading 'h42'")]),
        diag
    );
}

#[test]
fn no_diag_on_real_urls() {
    let doc =
        FakeDoc::mk_lines(&["# H1", "## H2", "[](www.bad.md)", "[](https://www.good.md)"]);

    let folder = FakeFolder::mk(vec![doc]);
    let diag = diag_to_human(&folder);

    assert_eq!(
        expected_pairs(&[("fake.md", "Link to non-existent document 'www.bad.md'")]),
        diag
    );
}

#[test]
fn markdown_extension_without_a_name_does_not_resolve_to_every_document() {
    let source = doc_lines("source.md", &["[x](.md)"]);

    let first = doc_lines("first.md", &["# First"]);

    let second = doc_lines("second.md", &["# Second"]);

    let folder = FakeFolder::mk(vec![source.clone(), first, second]);

    let entries = diag::check_doc(&folder, &source);
    match entries.as_slice() {
        [Entry::BrokenLink(..)] => {}
        diagnostic => panic!("Expected a broken link, got {diagnostic:?}"),
    }

    assert!(folder
        .filter_docs_by_intern_path(&InternPath::Approx(RelPath(".md".to_string())))
        .is_empty());
}

#[test]
fn no_diag_on_non_markdown_files() {
    let doc = FakeDoc::mk_lines(&[
        "# H1",
        "## H2",
        "[](bad.md)",
        "[](another%20bad.md)",
        "[](good/folder)",
    ]);

    let folder = FakeFolder::mk(vec![doc]);
    let diag = diag_to_human(&folder);

    assert_eq!(
        expected_pairs(&[
            ("fake.md", "Link to non-existent document 'bad.md'"),
            ("fake.md", "Link to non-existent document 'another bad.md'"),
        ]),
        diag
    );
}

#[test]
fn cross_file_diag_on_broken_wiki_links() {
    let doc = FakeDoc::mk_lines(&["[[bad]]"]);

    let folder = FakeFolder::mk(vec![doc]);
    let diag = diag_to_human(&folder);

    assert_eq!(
        expected_pairs(&[("fake.md", "Link to non-existent document 'bad'")]),
        diag
    );
}

#[test]
fn no_cross_file_diag_on_single_file_folders() {
    let doc = FakeDoc::mk_lines(&[
        "[](bad.md)", //
        "[[another-bad]]",
        "[bad-ref][bad-ref]",
    ]);

    let folder = Folder::single_file(doc.clone(), None);
    let diag = diag_to_human(&folder);

    assert_eq!(
        expected_pairs(&[(
            "fake.md",
            "Link to non-existent link definition with the label 'bad-ref'"
        )]),
        diag
    );
}

mod affected_document_tests {
    use super::*;

    fn doc(path: &str, lines: &[&str]) -> Doc {
        doc_lines(path, lines)
    }

    fn set_of(ids: &[&DocId]) -> BTreeSet<DocId> {
        ids.iter().map(|id| (*id).clone()).collect()
    }

    fn affected(before: &Folder, after: &Folder) -> BTreeSet<DocId> {
        let candidates = diag::affected_documents(
            &Workspace::of_folders(None, vec![before.clone()]),
            &Workspace::of_folders(None, vec![after.clone()]),
        )
        .get(&before.id())
        .cloned()
        .unwrap_or_default();

        let previous = full_diagnostics(before);
        let current = full_diagnostics(after);

        let mut keys: BTreeSet<DocId> = previous.keys().cloned().collect();
        keys.extend(current.keys().cloned());
        let changed_diagnostics: BTreeSet<DocId> = keys
            .into_iter()
            .filter(|doc_id| previous.get(doc_id) != current.get(doc_id))
            .collect();

        assert!(
            changed_diagnostics.is_subset(&candidates),
            "Documents with changed diagnostics were not selected: {:?}",
            &changed_diagnostics - &candidates
        );

        candidates
    }

    #[test]
    fn newly_available_target_affects_its_previously_broken_source() {
        let source = doc("source.md", &["[[Target#Section]]"]);
        let target = doc("target.md", &["# Target", "## Section"]);
        let before = FakeFolder::mk(vec![source.clone()]);
        let after = before.with_doc(target.clone());

        assert_eq!(set_of(&[source.id(), target.id()]), affected(&before, &after));
    }

    #[test]
    fn new_matching_target_affects_a_previously_unambiguous_source() {
        let source = doc("source.md", &["[[Target]]"]);
        let first = doc("one/target.md", &["# Target"]);
        let second = doc("two/target.md", &["# Target"]);
        let before = FakeFolder::mk(vec![source.clone(), first]);
        let after = before.with_doc(second.clone());

        assert!(diag::check_doc(&before, &source).is_empty());

        let entries = diag::check_doc(&after, &source);
        match entries.as_slice() {
            [Entry::AmbiguousLink(..)] => {}
            diagnostic => panic!("Expected an ambiguous link, got {diagnostic:?}"),
        }

        assert_eq!(set_of(&[source.id(), second.id()]), affected(&before, &after));
    }

    #[test]
    fn moving_a_target_heading_affects_incoming_references() {
        let source = doc("source.md", &["[[Target#Section]]"]);
        let target = doc("first.md", &["# Target", "## Section"]);
        let other = doc("second.md", &["# Target", "## Section"]);
        let moved = doc("first.md", &["# Target", "", "## Section"]);
        let before = FakeFolder::mk(vec![source.clone(), target.clone(), other]);
        let after = before.with_doc(moved);

        let related_line = |folder: &Folder| -> u32 {
            let entries = diag::check_doc(folder, &source);
            match entries.as_slice() {
                [Entry::AmbiguousLink(_, _, destinations)] => destinations
                    .iter()
                    .find(|destination| destination.doc().id() == target.id())
                    .expect("a destination pointing at the target")
                    .range()
                    .start
                    .line,
                diagnostic => panic!("Expected an ambiguous link, got {diagnostic:?}"),
            }
        };

        assert_eq!(1, related_line(&before));
        assert_eq!(2, related_line(&after));
        assert_eq!(set_of(&[source.id(), target.id()]), affected(&before, &after));
    }

    #[test]
    fn renaming_a_target_title_affects_references_to_its_old_alias() {
        let source = doc("source.md", &["[[Target]]"]);
        let target = doc("other.md", &["# Target"]);
        let renamed = doc("other.md", &["# Renamed"]);
        let before = FakeFolder::mk(vec![source.clone(), target.clone()]);
        let after = before.with_doc(renamed);

        assert!(diag::check_doc(&before, &source).is_empty());

        let entries = diag::check_doc(&after, &source);
        match entries.as_slice() {
            [Entry::BrokenLink(..)] => {}
            diagnostic => panic!("Expected a broken link, got {diagnostic:?}"),
        }

        assert_eq!(set_of(&[source.id(), target.id()]), affected(&before, &after));
    }

    #[test]
    fn removing_a_target_affects_its_incoming_references() {
        let source = doc("source.md", &["[[Target#Section]]"]);
        let target = doc("target.md", &["# Target", "## Section"]);
        let before = FakeFolder::mk(vec![source.clone(), target.clone()]);
        let after = before.without_doc(target.id()).unwrap();

        assert_eq!(set_of(&[source.id(), target.id()]), affected(&before, &after));
    }

    #[test]
    fn removing_one_of_two_targets_affects_the_previously_ambiguous_source() {
        let source = doc("source.md", &["[[Target#Section]]"]);
        let first = doc("first.md", &["# Target", "## Section"]);
        let second = doc("second.md", &["# Target", "## Section"]);
        let before = FakeFolder::mk(vec![source.clone(), first, second.clone()]);
        let after = before.without_doc(second.id()).unwrap();

        let entries = diag::check_doc(&before, &source);
        match entries.as_slice() {
            [Entry::AmbiguousLink(..)] => {}
            diagnostic => panic!("Expected an ambiguous link, got {diagnostic:?}"),
        }

        assert!(diag::check_doc(&after, &source).is_empty());
        assert_eq!(set_of(&[source.id(), second.id()]), affected(&before, &after));
    }

    #[test]
    fn unrelated_prose_edit_affects_only_the_edited_document() {
        let source = doc("source.md", &["[[Missing]]"]);
        let unrelated = doc("unrelated.md", &["Some prose."]);
        let edited = doc("unrelated.md", &["Some more prose."]);
        let before = FakeFolder::mk(vec![source, unrelated.clone()]);
        let after = before.with_doc(edited);

        assert_eq!(set_of(&[unrelated.id()]), affected(&before, &after));
    }

    #[test]
    fn unchanged_snapshots_and_reverted_edits_affect_no_documents() {
        let source = doc("source.md", &["[[Missing]]"]);
        let initial = FakeFolder::mk(vec![source.clone()]);
        let edited = initial.with_doc(doc("source.md", &["[[Target]]"]));
        let reverted = edited.with_doc(source);
        let target = doc("missing.md", &["# Missing"]);
        let added = initial.with_doc(target.clone());
        let removed = added.without_doc(target.id()).unwrap();

        assert!(affected(&initial, &initial).is_empty());
        assert!(affected(&initial, &reverted).is_empty());
        assert!(affected(&initial, &removed).is_empty());
    }

    #[test]
    fn a_version_change_alone_does_not_affect_diagnostics_of_an_already_open_document() {
        let source = doc("source.md", &["[[Missing]]"]);

        let open_at = |version: i32| {
            Doc::mk(
                &ParserSettings::default(),
                source.id().clone(),
                Some(version),
                source.text().clone(),
            )
            .expect("document parses")
        };

        let before = FakeFolder::mk(vec![open_at(1)]);
        let after = before.with_doc(open_at(2));

        assert!(affected(&before, &after).is_empty());
    }

    #[test]
    fn reopening_a_document_affects_it_even_when_its_text_is_unchanged() {
        let source = doc("source.md", &["[[Missing]]"]);

        let reopened = Doc::mk(
            &ParserSettings::default(),
            source.id().clone(),
            Some(1),
            source.text().clone(),
        )
        .expect("document parses");

        let before = FakeFolder::mk(vec![source.clone()]);
        let after = before.with_doc(reopened);

        assert_eq!(set_of(&[source.id()]), affected(&before, &after));
    }

    #[test]
    fn comparison_uses_latest_state_after_several_edits() {
        let source = doc("source.md", &["[[Target#Section]]"]);
        let initial = FakeFolder::mk(vec![source.clone()]);
        let intermediate = initial.with_doc(doc("target.md", &["# Target"]));
        let target = doc("target.md", &["# Target", "## Section"]);
        let latest = intermediate.with_doc(target.clone());

        assert_eq!(set_of(&[source.id(), target.id()]), affected(&initial, &latest));
    }

    #[test]
    fn reference_comparison_handles_added_and_removed_references_across_edits() {
        let unchanged = doc("a.md", &["[[Missing]]"]);
        let removed_reference = doc("b.md", &["[[Missing]]"]);
        let added_reference = doc("c.md", &["Some prose."]);
        let before =
            FakeFolder::mk(vec![unchanged, removed_reference.clone(), added_reference.clone()]);

        let after = before
            .with_doc(doc("b.md", &["Some prose."]))
            .with_doc(doc("c.md", &["[[Missing]]"]));

        assert_eq!(
            set_of(&[removed_reference.id(), added_reference.id()]),
            Conn::documents_with_changed_reference_resolutions(before.conn(), after.conn())
        );
    }

    #[test]
    fn folder_addition_and_removal_affect_all_its_documents() {
        let first = doc("first.md", &["[[Missing]]"]);
        let second = doc("second.md", &["Some prose."]);
        let folder = FakeFolder::mk(vec![first.clone(), second.clone()]);
        let empty: Workspace = Workspace::of_folders(None, Vec::<Folder>::new());
        let populated = Workspace::of_folders(None, vec![folder.clone()]);
        let expected = set_of(&[first.id(), second.id()]);

        assert_eq!(
            Some(&expected),
            diag::affected_documents(&empty, &populated).get(&folder.id())
        );

        assert_eq!(
            Some(&expected),
            diag::affected_documents(&populated, &empty).get(&folder.id())
        );
    }

    #[test]
    fn changing_folder_configuration_affects_all_its_documents() {
        let first = doc("first.md", &["[](missing.markdown)"]);
        let second = doc("second.md", &["Some prose."]);
        let before = FakeFolder::mk(vec![first.clone(), second.clone()]);
        let config = Config {
            core_markdown_file_extensions: Some(vec!["md".to_string()]),
            ..Config::default_config()
        };
        let after = before.with_config(Some(config));

        let entries = diag::check_doc(&before, &first);
        match entries.as_slice() {
            [Entry::BrokenLink(..)] => {}
            diagnostic => panic!("Expected a broken link, got {diagnostic:?}"),
        }

        assert!(diag::check_doc(&after, &first).is_empty());
        assert_eq!(set_of(&[first.id(), second.id()]), affected(&before, &after));
    }

    #[test]
    fn editing_one_folder_does_not_affect_another_folder() {
        let first_doc =
            FakeDoc::mk_with("[[Missing]]", "first/source.md", Some(&["first"]), None);

        let second_doc =
            FakeDoc::mk_with("Some prose.", "second/other.md", Some(&["second"]), None);

        let first_folder = Folder::multi_file(
            "first".to_string(),
            mk_folder_id(&dummy_root_path(&["first"])),
            vec![first_doc],
            None,
        );

        let second_folder = Folder::multi_file(
            "second".to_string(),
            mk_folder_id(&dummy_root_path(&["second"])),
            vec![second_doc.clone()],
            None,
        );

        let edited =
            FakeDoc::mk_with("More prose.", "second/other.md", Some(&["second"]), None);

        let before =
            Workspace::of_folders(None, vec![first_folder.clone(), second_folder.clone()]);

        let after = Workspace::of_folders(
            None,
            vec![first_folder, second_folder.with_doc(edited)],
        );

        let candidates = diag::affected_documents(&before, &after);

        let keys: BTreeSet<FolderId> = candidates.keys().cloned().collect();
        assert_eq!(
            BTreeSet::from([second_folder.id()]),
            keys
        );

        assert_eq!(
            Some(&set_of(&[second_doc.id()])),
            candidates.get(&second_folder.id())
        );
    }
}
