//! Port of `Tests/ConnDependencyTests.fs`.
//!
//! The F# original inspects the dependency graph indirectly, by counting oracle
//! invocations across an incremental `Conn.update`. The same technique is used
//! here: `CountingOracle` wraps a real `FolderOracle` and records how many times
//! each of the two oracle entry points is called.

mod common;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use common::{FakeDoc, FakeFolder};
use marksman::config::Config;
use marksman::conn::{
    CandidateDocumentResolution, Conn, ConnectionChange, DefinitionSelector, DocumentChange,
    DocumentInput, Oracle, OracleApi,
};
use marksman::doc::Doc;
use marksman::folder::Folder;
use marksman::misc::Slug;
use marksman::names::InternName;
use marksman::syms::{CrossRef, Def, Ref as SymRef, Scope, ScopedSym, Sym, Tag};

/// Reproduces `new System.Random(seed)` from .NET. Seeded `Random` instances
/// still use the legacy Knuth subtractive generator in .NET 6 and later, which
/// is what the original tests rely on for their edit sequences.
struct DotNetRandom {
    seed_array: [i32; 56],
    inext: usize,
    inext_p: usize,
}

impl DotNetRandom {
    const MBIG: i32 = i32::MAX;
    const MSEED: i32 = 161803398;

    fn new(seed: i32) -> DotNetRandom {
        let mut seed_array = [0i32; 56];
        let subtraction = if seed == i32::MIN { i32::MAX } else { seed.abs() };
        let mut mj = DotNetRandom::MSEED - subtraction;
        seed_array[55] = mj;
        let mut mk = 1;
        for i in 1..55usize {
            let ii = (21 * i) % 55;
            seed_array[ii] = mk;
            mk = mj - mk;
            if mk < 0 {
                mk += DotNetRandom::MBIG;
            }
            mj = seed_array[ii];
        }
        for _k in 1..5 {
            for i in 1..56usize {
                seed_array[i] -= seed_array[1 + ((i + 30) % 55)];
                if seed_array[i] < 0 {
                    seed_array[i] += DotNetRandom::MBIG;
                }
            }
        }
        DotNetRandom { seed_array, inext: 0, inext_p: 21 }
    }

    fn sample(&mut self) -> f64 {
        let mut loc_inext = self.inext + 1;
        if loc_inext >= 56 {
            loc_inext = 1;
        }
        let mut loc_inext_p = self.inext_p + 1;
        if loc_inext_p >= 56 {
            loc_inext_p = 1;
        }

        let mut ret_val = self.seed_array[loc_inext] - self.seed_array[loc_inext_p];
        if ret_val == DotNetRandom::MBIG {
            ret_val -= 1;
        }
        if ret_val < 0 {
            ret_val += DotNetRandom::MBIG;
        }

        self.seed_array[loc_inext] = ret_val;
        self.inext = loc_inext;
        self.inext_p = loc_inext_p;

        ret_val as f64 * (1.0 / DotNetRandom::MBIG as f64)
    }

    /// `Random.Next(maxValue)`, i.e. a value in `[0, maxValue)`.
    fn next(&mut self, max_value: i32) -> i32 {
        (self.sample() * max_value as f64) as i32
    }
}

fn config() -> Config {
    Config {
        core_incremental_references: Some(true),
        core_paranoid: Some(true),
        ..Config::default_config()
    }
}

fn doc(path: &str, lines: &[&str]) -> Doc {
    FakeDoc::mk_with(&lines.join("\n"), path, None, Some(config()))
}

fn folder(docs: Vec<Doc>) -> Folder {
    FakeFolder::mk_with_config(docs, Some(config()))
}

fn document_input(doc: &Doc) -> DocumentInput {
    DocumentInput {
        id: doc.id().clone(),
        slug: doc.slug(),
        path: doc.path_from_root(),
        symbols: doc.syms().clone(),
    }
}

/// The incremental graph must have the same observable results and internal
/// connection state as a clean construction.
fn assert_matches_clean_construction(folder: &Folder) {
    let expected = Conn::mk(&folder.oracle(), &folder.syms());
    let difference = Conn::difference(&expected, folder.conn());
    assert!(difference.is_empty(), "{}", difference.compact_format());
}

/// Count actual oracle calls, not timings or an implementation-maintained counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct OracleCallCounts {
    candidate_document_resolutions: usize,
    definition_selections: usize,
}

struct CountingOracle {
    inner: Oracle,
    candidate_document_resolutions: AtomicUsize,
    definition_selections: AtomicUsize,
}

impl CountingOracle {
    fn counts(&self) -> OracleCallCounts {
        OracleCallCounts {
            candidate_document_resolutions: self.candidate_document_resolutions.load(Ordering::SeqCst),
            definition_selections: self.definition_selections.load(Ordering::SeqCst),
        }
    }
}

impl OracleApi for CountingOracle {
    fn resolve_candidate_documents(&self, name: &InternName) -> CandidateDocumentResolution {
        self.candidate_document_resolutions.fetch_add(1, Ordering::SeqCst);
        self.inner.resolve_candidate_documents(name)
    }

    fn select_definitions(&self, selector: &DefinitionSelector) -> Vec<Def> {
        self.definition_selections.fetch_add(1, Ordering::SeqCst);
        self.inner.select_definitions(selector)
    }
}

fn update_with_oracle_call_counts(before: &Doc, after: &Doc, others: &[Doc]) -> OracleCallCounts {
    let mut old_docs = vec![before.clone()];
    old_docs.extend_from_slice(others);
    let old_folder = folder(old_docs);

    let mut new_docs = vec![after.clone()];
    new_docs.extend_from_slice(others);
    let new_folder = folder(new_docs);

    let counted: Arc<CountingOracle> = Arc::new(CountingOracle {
        inner: new_folder.oracle(),
        candidate_document_resolutions: AtomicUsize::new(0),
        definition_selections: AtomicUsize::new(0),
    });
    let counted_oracle: Oracle = counted.clone();

    let change = ConnectionChange::of_documents(
        &config().core_markdown_file_extensions(),
        &[DocumentChange::Replaced(document_input(before), document_input(after))],
    );

    let actual = Conn::update_incremental(old_folder.conn(), &counted_oracle, change);
    let diff = Conn::difference(new_folder.conn(), &actual);
    assert!(diff.is_empty(), "{}", diff.compact_format());

    counted.counts()
}

fn assert_counts(expected: OracleCallCounts, actual: OracleCallCounts) {
    assert_eq!(expected, actual);
}

// ---------------------------------------------------------------------------

/// `globalTagLookupTracksAddedAndRemovedSources`
#[test]
fn global_tag_lookup_tracks_added_and_removed_sources() {
    let first = doc("first.md", &["#tag"]);
    let second = doc("second.md", &["#tag"]);
    let tag = Sym::Tag(Tag("tag".to_string()));
    let sources = |folder: &Folder| -> BTreeSet<ScopedSym> {
        folder.conn().resolve(&(Scope::Global, tag.clone()))
    };
    let both = folder(vec![first.clone(), second.clone()]);

    assert_eq!(
        BTreeSet::from([
            (Scope::Doc(first.id().clone()), tag.clone()),
            (Scope::Doc(second.id().clone()), tag.clone())
        ]),
        sources(&both)
    );

    let without_first = both.with_doc(doc("first.md", &["No tag"]));
    assert_eq!(
        BTreeSet::from([(Scope::Doc(second.id().clone()), tag.clone())]),
        sources(&without_first)
    );

    let restored = without_first.with_doc(first.clone());
    assert_eq!(sources(&both), sources(&restored));
}

/// `removingMissingDocumentReferencesCleansDependencyState`
#[test]
fn removing_missing_document_references_cleans_dependency_state() {
    let source = |lines: &[&str]| doc("source.md", lines);

    let f = folder(vec![source(&["[[Absent]]", "[[Absent#One]]", "[[Absent#Two]]"])]);

    assert_matches_clean_construction(&f);
    let f = f.with_doc(source(&["[[Absent#Two]]"]));
    assert_matches_clean_construction(&f);
    let f = f.with_doc(source(&[]));
    assert_matches_clean_construction(&f);
}

/// `addingASectionToOneCandidateUpdatesResolution`
#[test]
fn adding_a_section_to_one_candidate_updates_resolution() {
    let source = doc("source.md", &["[[Alpha#Section]]"]);
    let a = doc("a.md", &["# Alpha", "## Section"]);
    let b = doc("b.md", &["# Alpha"]);
    let f = folder(vec![source.clone(), a.clone(), b.clone()]);
    assert_matches_clean_construction(&f);

    let scope = Scope::Doc(source.id().clone());
    let reference =
        SymRef::CrossRef(CrossRef::CrossSection("Alpha".to_string(), Slug::of_string("Section")));

    let f = f.with_doc(doc("b.md", &["# Alpha", "## Section"]));
    assert_matches_clean_construction(&f);
    let targets = f.conn().resolve(&(scope.clone(), Sym::Ref(reference.clone())));
    assert_eq!(2, targets.len());

    let f = f.without_doc(a.id()).unwrap();
    assert_matches_clean_construction(&f);

    let single = f.conn().resolve(&(scope.clone(), Sym::Ref(reference.clone())));
    assert_eq!(1, single.len());
}

/// `newPathCandidatesResolvePreviouslyMissingReferences`
#[test]
fn new_path_candidates_resolve_previously_missing_references() {
    let cases = [
        ("[[./a#Section]]", "group/a.md"),
        ("[[../a#Section]]", "a.md"),
        ("[[/a#Section]]", "a.md"),
        ("[[a#Section]]", "nested/a.md"),
        ("[[nested/a#Section]]", "nested/a.md"),
        ("[[nested\\a#Section]]", "nested/a.md"),
        ("[[space%20name#Section]]", "nested/space name.md"),
        ("[[a.md#Section]]", "nested/a.markdown"),
    ];

    for (link, target_path) in cases {
        let source = doc("group/source.md", &[link]);
        let target = doc(target_path, &["# Different title", "## Section"]);
        let f = folder(vec![source.clone()]).with_doc(target.clone());
        assert_matches_clean_construction(&f);

        let refs: Vec<&SymRef> = source.syms().iter().filter_map(Sym::as_ref).collect();
        assert_eq!(1, refs.len(), "link {link} should hold exactly one reference");
        let reference = refs[0].clone();

        let expected = BTreeSet::from([(
            Scope::Doc(target.id().clone()),
            Sym::Def(Def::Header(2, "section".to_string())),
        )]);

        assert_eq!(
            expected,
            f.conn().resolve(&(Scope::Doc(source.id().clone()), Sym::Ref(reference))),
            "link {link} -> {target_path}"
        );

        let removed = f.without_doc(target.id()).unwrap();
        assert_matches_clean_construction(&removed);
    }
}

/// `canonicalPathReplacementChangesDocumentIdentity`
#[test]
fn canonical_path_replacement_changes_document_identity() {
    let before = doc("a.md", &["# Alpha", "## Section"]);
    let after = doc("a.markdown", &["# Alpha", "## Section"]);
    let source = doc("source.md", &["[[a#Section]]", "[[Alpha]]"]);
    let f = folder(vec![source.clone(), before.clone()]).with_doc(after.clone());
    assert_matches_clean_construction(&f);

    // Removing via an equivalent canonical path must remove the actual owner.
    let removed = f.without_doc(before.id()).unwrap();
    assert_matches_clean_construction(&removed);
}

/// `documentWithoutTitleResolvesToDocumentDefinition`
#[test]
fn document_without_title_resolves_to_document_definition() {
    let target = doc("Alpha.md", &["Just some body text, no heading here."]);
    let source = doc("source.md", &["[[Alpha]]"]);
    let f = folder(vec![source.clone(), target.clone()]);
    let expected = BTreeSet::from([(Scope::Doc(target.id().clone()), Sym::Def(Def::Doc))]);

    assert_eq!(
        expected,
        f.conn().resolve(&(
            Scope::Doc(source.id().clone()),
            Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc("Alpha".to_string())))
        ))
    );

    assert_matches_clean_construction(&f);
}

/// `unlinkedRenameDoesNotEvaluateExistingReferences`
#[test]
fn unlinked_rename_does_not_evaluate_existing_references() {
    for size in [10usize, 1000] {
        let others: Vec<Doc> = (0..size)
            .map(|i| doc(&format!("doc{i}.md"), &["[[doc0]]", "[[doc0#Section]]", "## Section"]))
            .collect();

        let counts = update_with_oracle_call_counts(
            &doc("unlinked.md", &["# Before"]),
            &doc("unlinked.md", &["# After"]),
            &others,
        );

        assert_counts(
            OracleCallCounts { candidate_document_resolutions: 0, definition_selections: 0 },
            counts,
        );
    }
}

/// `addingHeadingDoesNotRetryMissingDocuments`
#[test]
fn adding_heading_does_not_retry_missing_documents() {
    for size in [10usize, 1000] {
        let others: Vec<Doc> =
            (0..size).map(|i| doc(&format!("doc{i}.md"), &["[[Absent#Section]]"])).collect();

        let counts = update_with_oracle_call_counts(
            &doc("a.md", &[]),
            &doc("a.md", &["## Section"]),
            &others,
        );

        assert_counts(
            OracleCallCounts { candidate_document_resolutions: 0, definition_selections: 0 },
            counts,
        );
    }
}

/// `headingEditReevaluatesOnlyReadersOfThatHeading`
#[test]
fn heading_edit_reevaluates_only_readers_of_that_heading() {
    let others = vec![doc("source.md", &["[[a#Section]]", "[[a#Other]]", "[[a]]"])];

    let counts = update_with_oracle_call_counts(
        &doc("a.md", &["## Other"]),
        &doc("a.md", &["## Other", "## Section"]),
        &others,
    );

    assert_counts(
        OracleCallCounts { candidate_document_resolutions: 0, definition_selections: 1 },
        counts,
    );
}

/// `aDefinitionSelectionIsSharedByAllDependentReferences`
#[test]
fn a_definition_selection_is_shared_by_all_dependent_references() {
    for size in [10usize, 1000] {
        let sources: Vec<Doc> =
            (0..size).map(|i| doc(&format!("source{i}.md"), &["[[a#Section]]"])).collect();

        let counts = update_with_oracle_call_counts(
            &doc("a.md", &["## Section"]),
            &doc("a.md", &["## Other"]),
            &sources,
        );

        assert_counts(
            OracleCallCounts { candidate_document_resolutions: 0, definition_selections: 1 },
            counts,
        );
    }
}

/// `editsAcrossParserConfigurations`
#[test]
fn edits_across_parser_configurations() {
    for title_from_heading in [true, false] {
        let cfg = Config {
            core_title_from_heading: Some(title_from_heading),
            core_markdown_file_extensions: Some(vec!["md".to_string(), "mdown".to_string()]),
            ..config()
        };

        let doc = |path: &str, lines: &[&str]| -> Doc {
            FakeDoc::mk_with(&lines.join("\n"), path, None, Some(cfg.clone()))
        };
        let folder = |docs: Vec<Doc>| FakeFolder::mk_with_config(docs, Some(cfg.clone()));

        let paths = ["nested/a.md", "a.mdown", "source.md", "other.md"];

        let contents: [&[&str]; 8] = [
            &["# Alpha", "## Section"],
            &["# Beta", "# Alpha", "## Other"],
            &["# Alpha", "# Beta", "## Other"],
            &["[[Alpha#Section]]", "[[a.mdown#Other]]", "[[nested/a#Section]]"],
            &["[link](./a.md#section)", "[[#Section]]", "## Section"],
            &["[label]", "", "[label]: /url"],
            &["[label]", "#tag"],
            &[],
        ];

        let mut random = DotNetRandom::new(456);
        let mut f = folder(Vec::new());

        for step in 1..=300 {
            let path = paths[random.next(paths.len() as i32) as usize];
            let d = doc(path, contents[random.next(contents.len() as i32) as usize]);

            f = if random.next(5) == 0 {
                f.without_doc(d.id()).unwrap()
            } else {
                f.with_doc(d)
            };

            let expected = Conn::mk(&f.oracle(), &f.syms());
            let difference = Conn::difference(&expected, f.conn());
            assert!(
                difference.is_empty(),
                "Parser configuration titleFromHeading={title_from_heading}, step={step}, path={path}: {}",
                difference.compact_format()
            );
        }
    }
}

/// `candidateDocumentsAreResolvedOnceForMultipleSections`
#[test]
fn candidate_documents_are_resolved_once_for_multiple_sections() {
    let counts = update_with_oracle_call_counts(
        &doc("a.md", &["# Before", "## One", "## Two"]),
        &doc("a.md", &["# After", "## One", "## Two"]),
        &[doc("source.md", &["[[Before]]", "[[Before#One]]", "[[Before#Two]]"])],
    );
    // Both dirty computations are evaluated once before the references detach
    // from the definition selection that is no longer reachable.
    assert_counts(
        OracleCallCounts { candidate_document_resolutions: 1, definition_selections: 1 },
        counts,
    );
}

/// `titleChangeUpdatesDefinitionSelectionWithoutResolvingCandidatesAgain`
#[test]
fn title_change_updates_definition_selection_without_resolving_candidates_again() {
    let counts = update_with_oracle_call_counts(
        &doc("a.md", &["# Before", "## Section"]),
        &doc("a.md", &["# After", "## Section"]),
        &[doc("source.md", &["[[a]]", "[[a#Section]]"])],
    );

    assert_counts(
        OracleCallCounts { candidate_document_resolutions: 0, definition_selections: 1 },
        counts,
    );
}

/// `batchChangesCanMoveCandidatesAndReplaceReferenceSources`
#[test]
fn batch_changes_can_move_candidates_and_replace_reference_sources() {
    let old_target = doc("old.md", &["# Alpha", "## Section"]);
    let new_target = doc("new.md", &["# Alpha", "## Section", "## Other"]);
    let old_source = doc("source.md", &["[[Alpha]]", "[[Alpha#Section]]"]);
    let new_source = doc("source.md", &["[[Alpha#Other]]", "[[Alpha#Section]]"]);
    let before = folder(vec![old_target.clone(), old_source.clone()]);
    let after = folder(vec![new_target.clone(), new_source.clone()]);

    let change = ConnectionChange::of_documents(
        &config().core_markdown_file_extensions(),
        &[
            DocumentChange::Removed(document_input(&old_target)),
            DocumentChange::Added(document_input(&new_target)),
            DocumentChange::Replaced(document_input(&old_source), document_input(&new_source)),
        ],
    );

    let actual = Conn::update_incremental(before.conn(), &after.oracle(), change);
    let diff = Conn::difference(after.conn(), &actual);
    assert!(diff.is_empty(), "{}", diff.compact_format());
    let reference =
        SymRef::CrossRef(CrossRef::CrossSection("Alpha".to_string(), Slug::of_string("Section")));

    let expected = BTreeSet::from([(
        Scope::Doc(new_target.id().clone()),
        Sym::Def(Def::Header(2, "section".to_string())),
    )]);

    assert_eq!(
        expected,
        actual.resolve(&(Scope::Doc(new_source.id().clone()), Sym::Ref(reference)))
    );
}

/// `changingExtensionsRebuildsCanonicalPathAliases`
#[test]
fn changing_extensions_rebuilds_canonical_path_aliases() {
    let before_config =
        Config { core_markdown_file_extensions: Some(vec!["mdown".to_string()]), ..config() };
    let target = doc("target.md", &["# Different", "## Section"]);
    let source = doc("source.md", &["[[target#Section]]", "[[/target.md#Section]]"]);

    let f = FakeFolder::mk_with_config(vec![target.clone(), source.clone()], Some(before_config))
        .with_config(Some(config()));

    let expected = BTreeSet::from([(
        Scope::Doc(target.id().clone()),
        Sym::Def(Def::Header(2, "section".to_string())),
    )]);

    for name in ["target", "/target.md"] {
        let reference =
            SymRef::CrossRef(CrossRef::CrossSection(name.to_string(), Slug::of_string("Section")));

        assert_eq!(
            expected,
            f.conn().resolve(&(Scope::Doc(source.id().clone()), Sym::Ref(reference)))
        );
    }

    assert_matches_clean_construction(&f);

    let removed = f.without_doc(target.id()).unwrap();
    assert_matches_clean_construction(&removed);
}
