//! Rename refactoring for headings, reference links and link definitions.
//!
//! Port of `Marksman.Refactor`.

use std::collections::BTreeMap;

use lsp_types::{
    DocumentChanges, Location, OptionalVersionedTextDocumentIdentifier, Range, TextDocumentEdit,
    TextEdit, Uri, WorkspaceEdit,
};

use crate::config::ComplWikiStyle;
use crate::cst::{self, Element, MdLink, MdLinkDef};
use crate::doc::Doc;
use crate::folder::Folder;
use crate::misc::{self, Slug};
use crate::names::WikiEncoded;
use crate::refs::{detect_file_link_kind, Dest, FileLinkKind};
use crate::syms::{CrossRef, Def, IntraRef, Ref as SymRef, Sym};
use lsp_types::Position;

#[derive(Clone, Debug)]
pub enum RenameResult {
    Edit(WorkspaceEdit),
    Error(String),
    Skip,
}

pub fn is_valid_label(name: &str) -> bool {
    !["\n", "[", "]", "(", ")"].iter().any(|bad| name.contains(bad))
}

pub fn is_valid_title(name: &str) -> bool {
    !["\n", "#"].iter().any(|bad| name.contains(bad))
}

/// Groups `(doc, element)` pairs by document, dropping duplicate elements.
fn group_by_first(pairs: Vec<(Doc, Element)>) -> Vec<(Doc, Vec<Element>)> {
    let mut grouped: BTreeMap<String, (Doc, Vec<Element>)> = BTreeMap::new();
    for (doc, el) in pairs {
        let entry = grouped
            .entry(doc.uri().to_string())
            .or_insert_with(|| (doc.clone(), Vec::new()));
        if !entry.1.contains(&el) {
            entry.1.push(el);
        }
    }
    grouped.into_values().collect()
}

fn cmp_text_edit_desc(a: &TextEdit, b: &TextEdit) -> std::cmp::Ordering {
    misc::cmp_range(&b.range, &a.range)
}

pub fn mk_workspace_edit(supports_document_edit: bool, doc_edits: Vec<TextDocumentEdit>) -> WorkspaceEdit {
    // Sort edits: those that affect the end of the document go first, so that
    // applying them does not invalidate earlier offsets.
    let mut doc_edits = doc_edits;
    for doc_edit in &mut doc_edits {
        doc_edit.edits.sort_by(|a, b| match (a, b) {
            (lsp_types::OneOf::Left(x), lsp_types::OneOf::Left(y)) => cmp_text_edit_desc(x, y),
            _ => std::cmp::Ordering::Equal,
        });
    }

    if supports_document_edit {
        WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(doc_edits)),
            change_annotations: None,
        }
    } else {
        let mut changes: BTreeMap<Uri, Vec<TextEdit>> = BTreeMap::new();
        for doc_edit in doc_edits {
            let edits: Vec<TextEdit> = doc_edit
                .edits
                .into_iter()
                .filter_map(|e| match e {
                    lsp_types::OneOf::Left(edit) => Some(edit),
                    _ => None,
                })
                .collect();
            changes.entry(doc_edit.text_document.uri).or_default().extend(edits);
        }
        WorkspaceEdit {
            changes: Some(changes.into_iter().collect()),
            document_changes: None,
            change_annotations: None,
        }
    }
}

pub fn rename_markdown_label(new_label: &str, element: &Element) -> Option<TextEdit> {
    match element {
        Element::ML(link) => link.data.reference_label().map(|label| TextEdit {
            range: label.range,
            new_text: new_label.to_string(),
        }),
        Element::MLD(node) => Some(TextEdit {
            range: node.data.label.range,
            new_text: new_label.to_string(),
        }),
        _ => None,
    }
}

fn lsp_doc(doc: &Doc) -> OptionalVersionedTextDocumentIdentifier {
    OptionalVersionedTextDocumentIdentifier {
        uri: crate::uri::parse(doc.uri()),
        version: doc.version(),
    }
}

pub fn rename_markdown_labels_in_doc(new_label: &str, doc: &Doc, els: &[Element]) -> TextDocumentEdit {
    let edits: Vec<TextEdit> = els
        .iter()
        .filter_map(|el| rename_markdown_label(new_label, el))
        .collect();

    TextDocumentEdit {
        text_document: lsp_doc(doc),
        edits: edits.into_iter().map(lsp_types::OneOf::Left).collect(),
    }
}

pub fn rename_heading_link(
    compl_style: ComplWikiStyle,
    src_doc: &Doc,
    src_heading: &Element,
    new_title: &str,
    target_doc: &Doc,
    target_el: &Element,
) -> Option<TextEdit> {
    let (_, src_id) = src_doc
        .structure()
        .try_find_symbol_for_concrete(src_heading)
        .and_then(Sym::as_def)
        .and_then(Def::as_header)
        .map(|(level, id)| (level, id.to_string()))
        .unwrap_or_else(|| panic!("Internal error: heading without a symbol: {src_heading:?}"));

    let should_rename = match target_doc.structure().try_find_symbol_for_concrete(target_el) {
        Some(Sym::Ref(SymRef::CrossRef(CrossRef::CrossDoc(doc_name)))) => {
            let link_kind =
                detect_file_link_kind(compl_style, target_doc.id(), doc_name, src_doc);
            link_kind == FileLinkKind::Title && misc::equal_slug_strings(&src_id, doc_name)
        }
        Some(Sym::Ref(SymRef::CrossRef(CrossRef::CrossSection(doc_name, section_name)))) => {
            if src_heading.is_title() {
                let link_kind =
                    detect_file_link_kind(compl_style, target_doc.id(), doc_name, src_doc);
                link_kind == FileLinkKind::Title && misc::equal_slug_strings(&src_id, doc_name)
            } else {
                Slug::of_string(&src_id) == *section_name
            }
        }
        Some(Sym::Ref(SymRef::IntraRef(IntraRef::IntraSection(section_name)))) => {
            Slug::of_string(&src_id) == *section_name
        }
        _ => false,
    };

    if !should_rename {
        return None;
    }

    match target_el {
        Element::WL(node) => {
            let wl = &node.data;
            let to_edit = if src_heading.is_title() { &wl.doc } else { &wl.heading };

            // TODO: consolidate with the completion logic
            let encode_fn: fn(&str) -> String = if src_heading.is_title() {
                match compl_style {
                    ComplWikiStyle::TitleSlug => |s| misc::slugify(s),
                    _ => WikiEncoded::encode_as_string,
                }
            } else {
                WikiEncoded::encode_as_string
            };

            let new_text = encode_fn(new_title);
            to_edit.as_ref().map(|node| TextEdit { range: node.range, new_text })
        }
        Element::ML(node) => match &node.data {
            MdLink::IL(_, url, _) => {
                let anchor = url.as_ref().and_then(|u| {
                    cst::split_url_node(u).1.map(|(_, range)| range)
                });
                let to_edit = if !src_heading.is_title() { anchor } else { None };
                to_edit.map(|range| TextEdit { range, new_text: misc::slugify(new_title) })
            }
            _ => None,
        },
        _ => None,
    }
}

pub fn rename_heading_links_in_doc(
    compl_style: ComplWikiStyle,
    src_doc: &Doc,
    src_heading: &Element,
    new_title: &str,
    target_doc: &Doc,
    target_els: &[Element],
) -> TextDocumentEdit {
    let edits: Vec<TextEdit> = target_els
        .iter()
        .filter_map(|target_el| {
            rename_heading_link(compl_style, src_doc, src_heading, new_title, target_doc, target_el)
        })
        .collect();

    TextDocumentEdit {
        text_document: lsp_doc(target_doc),
        edits: edits.into_iter().map(lsp_types::OneOf::Left).collect(),
    }
}

pub fn combine_document_edits(
    e1s: Vec<TextDocumentEdit>,
    e2s: Vec<TextDocumentEdit>,
) -> Vec<TextDocumentEdit> {
    fn deconstruct(x: TextDocumentEdit) -> (OptionalVersionedTextDocumentIdentifier, Vec<lsp_types::OneOf<TextEdit, lsp_types::AnnotatedTextEdit>>) {
        (x.text_document, x.edits)
    }

    let mut by_doc: BTreeMap<String, (OptionalVersionedTextDocumentIdentifier, Vec<_>)> =
        BTreeMap::new();

    for edit in e2s.into_iter().chain(e1s).map(deconstruct) {
        let key = edit.0.uri.to_string();
        by_doc.entry(key).or_insert_with(|| (edit.0.clone(), Vec::new())).1.extend(edit.1);
    }

    by_doc
        .into_values()
        .map(|(doc, edits)| TextDocumentEdit { text_document: doc, edits })
        .collect()
}

pub fn rename(
    supports_document_edit: bool,
    folder: &Folder,
    src_doc: &Doc,
    pos: Position,
    new_name: &str,
) -> RenameResult {
    let Some(el) = src_doc.cst().element_at_pos(pos).cloned() else {
        return RenameResult::Skip;
    };

    match &el {
        Element::ML(link) => {
            let Some(label) = link.data.reference_label() else {
                return RenameResult::Skip;
            };
            if !is_valid_label(new_name) {
                return RenameResult::Error(format!("Not a valid label name: {new_name}"));
            }
            if !misc::range_contains_inclusive(&label.range, pos) {
                return RenameResult::Skip;
            }

            let refs = Dest::find_element_refs(true, folder, src_doc, &el);
            // With reference link labels there is no ambiguity about the
            // destination, so the element's destination need not be inspected.
            let doc_edits: Vec<TextDocumentEdit> = group_by_first(refs)
                .into_iter()
                .map(|(doc, els)| rename_markdown_labels_in_doc(new_name, &doc, &els))
                .collect();

            RenameResult::Edit(mk_workspace_edit(supports_document_edit, doc_edits))
        }
        Element::MLD(node) => {
            if !is_valid_label(new_name) {
                return RenameResult::Error(format!("Not a valid label name: {new_name}"));
            }
            if !misc::range_contains_inclusive(&node.data.label.range, pos) {
                return RenameResult::Skip;
            }

            let refs = Dest::find_element_refs(true, folder, src_doc, &el);
            let doc_edits: Vec<TextDocumentEdit> = group_by_first(refs)
                .into_iter()
                .map(|(doc, els)| rename_markdown_labels_in_doc(new_name, &doc, &els))
                .collect();

            RenameResult::Edit(mk_workspace_edit(supports_document_edit, doc_edits))
        }
        Element::H(heading) => {
            if !is_valid_title(new_name) {
                return RenameResult::Error(format!("Not a valid title: {new_name}"));
            }
            if !misc::range_contains_inclusive(&heading.data.title.range, pos) {
                return RenameResult::Skip;
            }

            let heading_edit = TextDocumentEdit {
                text_document: lsp_doc(src_doc),
                edits: vec![lsp_types::OneOf::Left(TextEdit {
                    range: heading.data.title.range,
                    new_text: new_name.to_string(),
                })],
            };

            let refs = Dest::find_element_refs(false, folder, src_doc, &el);
            let compl_style = folder.config_or_default().compl_wiki_style();

            let link_edits: Vec<TextDocumentEdit> = group_by_first(refs)
                .into_iter()
                .map(|(target_doc, target_els)| {
                    rename_heading_links_in_doc(
                        compl_style,
                        src_doc,
                        &el,
                        new_name,
                        &target_doc,
                        &target_els,
                    )
                })
                .collect();

            let doc_edits = combine_document_edits(link_edits, vec![heading_edit]);
            RenameResult::Edit(mk_workspace_edit(supports_document_edit, doc_edits))
        }
        _ => RenameResult::Skip,
    }
}

/// The range a rename at `pos` would apply to, used for prepare-rename.
pub fn rename_range(src_doc: &Doc, pos: Position) -> Option<Range> {
    let el = src_doc.cst().element_at_pos(pos)?;
    match el {
        Element::ML(link) => {
            let label = link.data.reference_label()?;
            if misc::range_contains_inclusive(&label.range, pos) {
                Some(label.range)
            } else {
                None
            }
        }
        Element::MLD(node) => {
            let range = node.data.label.range;
            if misc::range_contains_inclusive(&range, pos) {
                Some(range)
            } else {
                None
            }
        }
        Element::H(heading) => {
            let range = heading.data.title.range;
            if misc::range_contains_inclusive(&range, pos) {
                Some(range)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Re-exported for the server layer, which needs the same helper.
pub fn location(uri: &str, range: Range) -> Location {
    Location { uri: crate::uri::parse(uri), range }
}

/// Link definition data used by rename; re-exported to keep the CST import local.
pub type CstMdLinkDef = MdLinkDef;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::FolderId;
    use crate::text::mk_text;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-refactor-{name}"));
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

    fn edits_of(result: &RenameResult) -> Vec<(String, Range, String)> {
        match result {
            RenameResult::Edit(edit) => match &edit.document_changes {
                Some(DocumentChanges::Edits(doc_edits)) => doc_edits
                    .iter()
                    .flat_map(|de| {
                        de.edits.iter().filter_map(|e| match e {
                            lsp_types::OneOf::Left(t) => {
                                Some((de.text_document.uri.to_string(), t.range, t.new_text.clone()))
                            }
                            _ => None,
                        })
                    })
                    .collect(),
                _ => Vec::new(),
            },
            other => panic!("expected an edit, got {other:?}"),
        }
    }

    #[test]
    fn label_and_title_validity() {
        assert!(is_valid_label("good label"));
        assert!(is_valid_label("Title #1 is fine as a label"));
        assert!(!is_valid_label("bad[label"));
        assert!(!is_valid_label("bad(label"));
        assert!(!is_valid_label("bad\nlabel"));

        assert!(is_valid_title("A plain title"));
        // A '#' would be read as a heading anchor separator.
        assert!(!is_valid_title("Title #1"));
        assert!(!is_valid_title("Multi\nline"));
    }

    #[test]
    fn renames_a_heading_and_its_wiki_links() {
        let dir = temp_dir("heading");
        write(&dir, "target.md", "# Old Title\n");
        write(&dir, "a.md", "# A\n\n[[Old Title]]\n");
        let folder = load(&dir);
        let target = doc_named(&folder, "Old Title");

        let result = rename(
            true,
            &folder,
            &target,
            Position { line: 0, character: 4 },
            "New Title",
        );
        let edits = edits_of(&result);

        assert_eq!(edits.len(), 2);
        assert!(edits.iter().any(|(_, _, t)| t == "New Title"));
        assert!(edits.iter().any(|(_, _, t)| t == "new-title"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renames_reference_link_labels_everywhere() {
        let dir = temp_dir("labels");
        write(&dir, "a.md", "See [lab] and [lab].\n\n[lab]: /url\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "a");

        let result = rename(true, &folder, &doc, Position { line: 2, character: 2 }, "newlab");
        let edits = edits_of(&result);
        assert_eq!(edits.len(), 3);
        assert!(edits.iter().all(|(_, _, t)| t == "newlab"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn invalid_new_label_is_an_error() {
        let dir = temp_dir("invalid");
        write(&dir, "a.md", "See [lab].\n\n[lab]: /url\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "a");

        match rename(true, &folder, &doc, Position { line: 2, character: 2 }, "bad[label") {
            RenameResult::Error(msg) => assert_eq!(msg, "Not a valid label name: bad[label"),
            other => panic!("expected an error, got {other:?}"),
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_outside_a_renammable_element_is_skipped() {
        let dir = temp_dir("skip");
        write(&dir, "a.md", "just prose\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "a");

        assert!(matches!(
            rename(true, &folder, &doc, Position { line: 0, character: 2 }, "x"),
            RenameResult::Skip
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prepare_rename_reports_the_label_range() {
        let doc = Doc::mk(
            &crate::config::ParserSettings::default(),
            crate::names::DocId::mk_rooted(
                &FolderId::of_uri("file:///w"),
                crate::paths::LocalPath::of_system("/w/a.md"),
            ),
            Some(1),
            mk_text("# Heading\n\nSee [lab].\n\n[lab]: /url\n"),
        )
        .unwrap();

        let heading_range = rename_range(&doc, Position { line: 0, character: 4 });
        assert_eq!(heading_range, Some(Range::new(Position::new(0, 2), Position::new(0, 9))));

        let label_range = rename_range(&doc, Position { line: 4, character: 2 });
        assert_eq!(label_range, Some(Range::new(Position::new(4, 1), Position::new(4, 4))));

        assert!(rename_range(&doc, Position { line: 2, character: 0 }).is_none());
    }

    #[test]
    fn edits_are_ordered_from_the_end_of_the_document() {
        let edit = |line: u32| TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: crate::uri::parse("file:///w/a.md"),
                version: None,
            },
            edits: vec![
                lsp_types::OneOf::Left(TextEdit {
                    range: Range::new(Position::new(0, 0), Position::new(0, 1)),
                    new_text: "first".into(),
                }),
                lsp_types::OneOf::Left(TextEdit {
                    range: Range::new(Position::new(line, 0), Position::new(line, 1)),
                    new_text: "last".into(),
                }),
            ],
        };

        let ws = mk_workspace_edit(true, vec![edit(5)]);
        match ws.document_changes {
            Some(DocumentChanges::Edits(edits)) => {
                let first = match &edits[0].edits[0] {
                    lsp_types::OneOf::Left(t) => t.new_text.clone(),
                    _ => panic!("expected a text edit"),
                };
                assert_eq!(first, "last");
            }
            _ => panic!("expected document changes"),
        }
    }

    #[test]
    fn clients_without_document_edits_get_plain_changes() {
        let doc_edit = TextDocumentEdit {
            text_document: OptionalVersionedTextDocumentIdentifier {
                uri: crate::uri::parse("file:///w/a.md"),
                version: None,
            },
            edits: vec![lsp_types::OneOf::Left(TextEdit {
                range: Range::default(),
                new_text: "x".into(),
            })],
        };
        let ws = mk_workspace_edit(false, vec![doc_edit]);
        assert!(ws.document_changes.is_none());
        assert_eq!(ws.changes.unwrap().len(), 1);
    }
}
