//! Diagnostics: broken links, ambiguous links and non-breaking whitespace.
//!
//! Port of `Marksman.Diag`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, NumberOrString, Range, Position,
};

use crate::conn::Conn;
use crate::cst::{Element, MdLink};
use crate::doc::Doc;
use crate::folder::Folder;
use crate::misc;
use crate::names::{DocId, FolderId};
use crate::paths::UriWith;
use crate::refs::Dest;
use crate::syms::{CrossRef, IntraRef, Ref as SymRef, Scope, Sym};
use crate::workspace::Workspace;

#[derive(Clone, Debug)]
pub enum Entry {
    AmbiguousLink(Element, SymRef, Vec<Dest>),
    BrokenLink(Element, SymRef),
    NonBreakableWhitespace(Range),
}

pub fn code(entry: &Entry) -> &'static str {
    match entry {
        Entry::AmbiguousLink(..) => "1",
        Entry::BrokenLink(..) => "2",
        Entry::NonBreakableWhitespace(_) => "3",
    }
}

const NON_BREAKING_WHITESPACE: &str = "\u{00a0}";

/// A heading written with a non-breaking space after the hashes is not a heading.
pub fn check_non_breaking_whitespace(doc: &Doc) -> Vec<Entry> {
    let headings: Vec<String> = (1..=7).map(|n| "#".repeat(n)).collect();
    let text = doc.text();

    let mut out = Vec::new();
    for x in 0..=text.line_map.num_lines() {
        let line = text.line_content(x);
        let heading_like = headings
            .iter()
            .find(|h| line.starts_with(&format!("{h}{NON_BREAKING_WHITESPACE}")));

        if let Some(heading) = heading_like {
            let len = heading.chars().count() as u32;
            out.push(Entry::NonBreakableWhitespace(Range {
                start: Position { line: x as u32, character: len },
                end: Position { line: x as u32, character: len + 1 },
            }));
        }
    }
    out
}

pub fn check_link(folder: &Folder, doc: &Doc, link_el: &Element) -> Vec<Entry> {
    let exts = folder.configured_markdown_exts();

    let Some(r) = doc
        .structure()
        .try_find_symbol_for_concrete(link_el)
        .and_then(Sym::as_ref)
        .cloned()
    else {
        return Vec::new();
    };

    let refs = Dest::try_resolve_element(folder, doc, link_el);

    if folder.is_single_file() && r.is_cross() {
        Vec::new()
    } else if refs.len() == 1 {
        Vec::new()
    } else if refs.is_empty() {
        match link_el {
            // Inline shortcut links are often a part of regular text; raising
            // diagnostics on them would be noisy.
            Element::ML(node) if matches!(node.data, MdLink::RS(_)) => Vec::new(),
            Element::ML(node) => match &node.data {
                MdLink::IL(_, Some(url), _) => {
                    if misc::is_markdown_file(&exts, &url.data.decode()) {
                        vec![Entry::BrokenLink(link_el.clone(), r)]
                    } else {
                        Vec::new()
                    }
                }
                _ => vec![Entry::BrokenLink(link_el.clone(), r)],
            },
            _ => vec![Entry::BrokenLink(link_el.clone(), r)],
        }
    } else {
        vec![Entry::AmbiguousLink(link_el.clone(), r, refs)]
    }
}

pub fn check_links(folder: &Folder, doc: &Doc) -> Vec<Entry> {
    let links: Vec<Element> = doc.index().links().collect();
    links.iter().flat_map(|l| check_link(folder, doc, l)).collect()
}

pub fn check_doc(folder: &Folder, doc: &Doc) -> Vec<Entry> {
    let mut out = check_links(folder, doc);
    out.extend(check_non_breaking_whitespace(doc));
    out
}

pub fn dest_to_human(dest: &Dest) -> String {
    match dest {
        Dest::Doc(fl) => format!("document {}", fl.doc.name()),
        Dest::Heading(doc_link, heading) => format!(
            "heading {} in the document {}",
            heading.data.name(),
            doc_link.doc().name()
        ),
        Dest::LinkDef(_, ld) => format!("link definition {}", ld.data.name()),
        Dest::Tag(doc, tag) => format!("tag {} in the document {}", tag.data.name.text, doc.name()),
    }
}

pub fn doc_to_human(name: &str) -> String {
    format!("document '{name}'")
}

pub fn ref_to_human(r: &SymRef) -> String {
    match r {
        SymRef::CrossRef(CrossRef::CrossDoc(doc_name)) => doc_to_human(doc_name),
        SymRef::CrossRef(CrossRef::CrossSection(doc_name, section_name)) => {
            format!("heading '{section_name}' in {}", doc_to_human(doc_name))
        }
        SymRef::IntraRef(IntraRef::IntraSection(heading)) => format!("heading '{heading}'"),
        SymRef::IntraRef(IntraRef::IntraLinkDef(ld)) => {
            format!("link definition with the label '{ld}'")
        }
    }
}

fn severity_of(el: &Element) -> DiagnosticSeverity {
    match el {
        Element::WL(_) => DiagnosticSeverity::ERROR,
        Element::ML(_) => DiagnosticSeverity::WARNING,
        Element::H(_) | Element::MLD(_) | Element::T(_) | Element::YML(_) => {
            DiagnosticSeverity::INFORMATION
        }
    }
}

pub fn diag_to_lsp(diag: &Entry) -> Diagnostic {
    match diag {
        Entry::AmbiguousLink(el, r, dests) => {
            let related: Vec<DiagnosticRelatedInformation> = dests
                .iter()
                .map(|dest| DiagnosticRelatedInformation {
                    location: dest.location(),
                    message: format!("Duplicate definition of {}", ref_to_human(r)),
                })
                .collect();

            Diagnostic {
                range: el.range(),
                severity: Some(severity_of(el)),
                code: Some(NumberOrString::String(code(diag).to_string())),
                code_description: None,
                source: Some("Marksman".to_string()),
                message: format!("Ambiguous link to {}", ref_to_human(r)),
                related_information: Some(related),
                tags: None,
                data: None,
            }
        }
        Entry::BrokenLink(el, r) => Diagnostic {
            range: el.range(),
            severity: Some(severity_of(el)),
            code: Some(NumberOrString::String(code(diag).to_string())),
            code_description: None,
            source: Some("Marksman".to_string()),
            message: format!("Link to non-existent {}", ref_to_human(r)),
            related_information: None,
            tags: None,
            data: None,
        },
        Entry::NonBreakableWhitespace(range) => Diagnostic {
            range: *range,
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String(code(diag).to_string())),
            code_description: None,
            source: Some("Marksman".to_string()),
            message: "Non-breaking whitespace used instead of regular whitespace. This line won't be interpreted as a heading".to_string(),
            related_information: None,
            tags: None,
            data: None,
        },
    }
}

/// Diagnostics are `Arc`-shared so that a document unaffected by an edit keeps
/// the very same vector, which is what the original's `array<Diagnostic>`
/// reference identity gives it.
pub type FolderDiag = BTreeMap<DocId, Arc<Vec<Diagnostic>>>;
pub type WorkspaceDiag = BTreeMap<FolderId, FolderDiag>;

/// Documents whose link resolutions point back at any of `doc_ids`. Their
/// diagnostics can change when the targets change, even though they were not
/// edited.
fn incoming_reference_documents(folder: &Folder, doc_ids: &BTreeSet<DocId>) -> BTreeSet<DocId> {
    let mut sources = BTreeSet::new();

    for doc_id in doc_ids {
        let doc = folder.find_doc_by_id(doc_id);
        for definition in doc.syms().iter().filter_map(Sym::as_def) {
            for (scope, sym) in folder
                .conn()
                .resolve(&(Scope::Doc(doc_id.clone()), Sym::Def(definition.clone())))
            {
                if let (Scope::Doc(source), Sym::Ref(_)) = (scope, sym) {
                    sources.insert(source);
                }
            }
        }
    }

    sources
}

/// Documents whose diagnostics may differ between two immutable workspace
/// states. The comparison runs at publication time, so edits coalesced by the
/// diagnostics publisher need no separate change log.
pub fn affected_documents(before: &Workspace, after: &Workspace) -> BTreeMap<FolderId, BTreeSet<DocId>> {
    let previous_folders: BTreeMap<FolderId, Folder> =
        before.folders().map(|f| (f.id(), f.clone())).collect();
    let current_folders: BTreeMap<FolderId, Folder> =
        after.folders().map(|f| (f.id(), f.clone())).collect();

    let mut folder_ids: BTreeSet<FolderId> = previous_folders.keys().cloned().collect();
    folder_ids.extend(current_folders.keys().cloned());

    let mut out = BTreeMap::new();
    for folder_id in folder_ids {
        let previous = previous_folders.get(&folder_id);
        let current = current_folders.get(&folder_id);

        let affected: BTreeSet<DocId> = match (previous, current) {
            (Some(old), Some(new)) if old.same_snapshot(new) => BTreeSet::new(),
            (Some(old), Some(new))
                if old.config() != new.config() || old.is_single_file() != new.is_single_file() =>
            {
                let mut all: BTreeSet<DocId> = old.docs().iter().map(|d| d.id().clone()).collect();
                all.extend(new.docs().iter().map(|d| d.id().clone()));
                all
            }
            (Some(old), Some(new)) => {
                let docs = Folder::docs_difference(old, new);

                let old_targets: BTreeSet<DocId> =
                    docs.changed.union(&docs.removed).cloned().collect();
                let new_targets: BTreeSet<DocId> =
                    docs.changed.union(&docs.added).cloned().collect();

                // An unchanged graph is the same allocation, so there is
                // nothing to compare. Otherwise the shared computed-value
                // partitions keep the traversal to what actually changed.
                let changed_resolutions = if Arc::ptr_eq(old.conn_arc(), new.conn_arc()) {
                    Default::default()
                } else {
                    Conn::documents_with_changed_reference_resolutions(old.conn(), new.conn())
                };

                let mut affected = BTreeSet::new();
                affected.extend(docs.added.iter().cloned());
                affected.extend(docs.removed.iter().cloned());
                affected.extend(docs.changed.iter().cloned());
                affected.extend(docs.reopened.iter().cloned());
                affected.extend(changed_resolutions);
                affected.extend(incoming_reference_documents(old, &old_targets));
                affected.extend(incoming_reference_documents(new, &new_targets));
                affected
            }
            (Some(folder), None) | (None, Some(folder)) => {
                folder.docs().iter().map(|d| d.id().clone()).collect()
            }
            (None, None) => BTreeSet::new(),
        };

        if !affected.is_empty() {
            out.insert(folder_id, affected);
        }
    }

    out
}

/// Calculates diagnostics for the current workspace. Without prior results every
/// document is recalculated; otherwise only affected documents are.
pub fn calculate(
    previous: Option<&(Workspace, WorkspaceDiag)>,
    current: &Workspace,
) -> (WorkspaceDiag, BTreeMap<FolderId, BTreeSet<DocId>>) {
    let affected = match previous {
        None => current
            .folders()
            .map(|folder| {
                (
                    folder.id(),
                    folder.docs().iter().map(|d| d.id().clone()).collect::<BTreeSet<_>>(),
                )
            })
            .collect(),
        Some((previous_workspace, _)) => affected_documents(previous_workspace, current),
    };

    let previous_diagnostics: &WorkspaceDiag = match previous {
        Some((_, diags)) => diags,
        None => &BTreeMap::new(),
    };

    let mut diagnostics = BTreeMap::new();
    for folder in current.folders() {
        let folder_id = folder.id();
        let previous_folder = previous_diagnostics.get(&folder_id).cloned().unwrap_or_default();
        let affected_ids = affected.get(&folder_id).cloned().unwrap_or_default();

        let mut updated = previous_folder;
        for doc_id in affected_ids {
            let doc_path: UriWith<crate::paths::LocalPath> = doc_id.raw().rooted_rel_to_abs();
            let abs = match &doc_path.data {
                crate::paths::LocalPath::Abs(p) => p.clone(),
                crate::paths::LocalPath::Rel(_) => continue,
            };

            match folder.try_find_doc_by_path(&abs) {
                None => {
                    updated.remove(&doc_id);
                }
                Some(doc) => {
                    let diags: Vec<Diagnostic> =
                        check_doc(folder, &doc).iter().map(diag_to_lsp).collect();
                    updated.insert(doc_id, Arc::new(diags));
                }
            }
        }

        diagnostics.insert(folder_id, updated);
    }

    (diagnostics, affected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::mk_text;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-diag-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(dir: &std::path::Path, rel: &str, content: &str) {
        let path = dir.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn load(dir: &std::path::Path) -> Folder {
        let id = FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap()));
        Folder::try_load(None, "test", id).unwrap()
    }

    fn doc_named(folder: &Folder, name: &str) -> Doc {
        folder.docs().into_iter().find(|d| d.name() == name).unwrap()
    }

    #[test]
    fn broken_wiki_link_is_an_error() {
        let dir = temp_dir("broken");
        write(&dir, "a.md", "# A\n\n[[Missing]]\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let entries = check_doc(&folder, &doc);
        assert_eq!(entries.len(), 1);
        assert!(matches!(entries[0], Entry::BrokenLink(..)));

        let diag = diag_to_lsp(&entries[0]);
        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert_eq!(diag.message, "Link to non-existent document 'Missing'");
        assert_eq!(diag.source.as_deref(), Some("Marksman"));
        assert_eq!(diag.code, Some(NumberOrString::String("2".to_string())));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolved_link_produces_nothing() {
        let dir = temp_dir("resolved");
        write(&dir, "a.md", "# A\n\n[[B]]\n");
        write(&dir, "b.md", "# B\n");
        let folder = load(&dir);
        assert!(check_doc(&folder, &doc_named(&folder, "A")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn ambiguous_link_is_reported_with_related_locations() {
        let dir = temp_dir("ambiguous");
        write(&dir, "a.md", "# A\n\n[[Note]]\n");
        write(&dir, "one/Note.md", "# Note\n");
        write(&dir, "two/Note.md", "# Note\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let entries = check_doc(&folder, &doc);
        assert_eq!(entries.len(), 1);
        let diag = diag_to_lsp(&entries[0]);
        assert_eq!(diag.message, "Ambiguous link to document 'Note'");
        assert_eq!(diag.related_information.unwrap().len(), 2);
        assert_eq!(diag.code, Some(NumberOrString::String("1".to_string())));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn shortcut_reference_links_are_not_reported_as_broken() {
        let dir = temp_dir("shortcut");
        write(&dir, "a.md", "# A\n\nSome [text] here.\n");
        let folder = load(&dir);
        assert!(check_doc(&folder, &doc_named(&folder, "A")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn inline_links_to_non_markdown_files_are_quiet() {
        let dir = temp_dir("inline");
        write(&dir, "a.md", "# A\n\n[pic](missing.png) and [doc](missing.md)\n");
        let folder = load(&dir);
        let entries = check_doc(&folder, &doc_named(&folder, "A"));

        // Only the .md destination is reported; the image is left alone.
        assert_eq!(entries.len(), 1);
        match &entries[0] {
            Entry::BrokenLink(_, SymRef::CrossRef(CrossRef::CrossDoc(d))) => {
                assert_eq!(d, "missing.md")
            }
            other => panic!("expected a broken link to missing.md, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn non_breaking_whitespace_in_a_heading_is_flagged() {
        let dir = temp_dir("nbsp");
        write(&dir, "a.md", "#\u{00a0}Title\n\n## Fine\n");
        let folder = load(&dir);
        let entries = check_doc(&folder, &doc_named(&folder, "a"));

        assert_eq!(entries.len(), 1);
        let diag = diag_to_lsp(&entries[0]);
        assert_eq!(diag.code, Some(NumberOrString::String("3".to_string())));
        assert_eq!(diag.range.start.line, 0);
        assert_eq!(diag.range.start.character, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cross_refs_are_not_checked_in_single_file_folders() {
        let doc = Doc::mk(
            &crate::config::ParserSettings::default(),
            DocId::mk_rooted(&FolderId::of_uri("file:///w"), crate::paths::LocalPath::of_system("/w/a.md")),
            Some(1),
            mk_text("# A\n\n[[Missing]]\n"),
        )
        .unwrap();
        let folder = Folder::single_file(doc.clone(), None);
        assert!(check_doc(&folder, &doc).is_empty());
    }

    #[test]
    fn human_descriptions_name_the_target() {
        assert_eq!(doc_to_human("note"), "document 'note'");
        assert_eq!(
            ref_to_human(&SymRef::CrossRef(CrossRef::CrossSection(
                "note".into(),
                misc::Slug::of_string("A Section")
            ))),
            "heading 'a-section' in document 'note'"
        );
        assert_eq!(
            ref_to_human(&SymRef::IntraRef(IntraRef::IntraSection(misc::Slug::of_string("X")))),
            "heading 'x'"
        );
        assert_eq!(
            ref_to_human(&SymRef::IntraRef(IntraRef::IntraLinkDef(misc::LinkLabel::of_string("L")))),
            "link definition with the label 'l'"
        );
    }

    #[test]
    fn calculate_recomputes_everything_without_a_previous_state() {
        let dir = temp_dir("calc");
        write(&dir, "a.md", "# A\n\n[[Missing]]\n");
        write(&dir, "b.md", "# B\n");
        let ws = Workspace::of_folders(None, vec![load(&dir)]);

        let (diags, affected) = calculate(None, &ws);
        assert_eq!(affected.values().next().unwrap().len(), 2);
        let folder_diags = diags.values().next().unwrap();
        let total: usize = folder_diags.values().map(|d| d.len()).sum();
        assert_eq!(total, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn affected_documents_include_documents_linking_to_a_changed_one() {
        let dir = temp_dir("affected");
        write(&dir, "a.md", "# A\n\n[[Target]]\n");
        write(&dir, "target.md", "# Target\n");
        let before = Workspace::of_folders(None, vec![load(&dir)]);

        write(&dir, "target.md", "# Target renamed\n");
        let after = Workspace::of_folders(None, vec![load(&dir)]);

        let affected = affected_documents(&before, &after);
        let ids = affected.values().next().unwrap();
        assert_eq!(ids.len(), 2, "both the edited doc and its referrer");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unchanged_workspace_affects_nothing() {
        let dir = temp_dir("unchanged");
        write(&dir, "a.md", "# A\n");
        let ws = Workspace::of_folders(None, vec![load(&dir)]);
        assert!(affected_documents(&ws, &ws).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

}
