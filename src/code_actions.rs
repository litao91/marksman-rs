//! Code actions: generate a table of contents, create a missing linked file.
//!
//! Port of `Marksman.CodeActions`.

use std::collections::HashMap;

use lsp_types::{
    CreateFile, Range, ResourceOp, TextEdit, Uri, WorkspaceEdit,
};

use crate::config::Config;
use crate::doc::Doc;
use crate::folder::Folder;
use crate::misc::{self, ensure_markdown_ext};
use crate::names::InternName;
use crate::paths::{AbsPath, RelPath};
use crate::syms::{Ref as SymRef, Sym};
use crate::text::DOCUMENT_BEGINNING;
use crate::toc::{InsertionPoint, TableOfContents};

#[derive(Clone, Debug)]
pub struct DocumentAction {
    pub name: String,
    pub new_text: String,
    pub edit: Range,
}

#[derive(Clone, Debug)]
pub struct CreateFileAction {
    pub name: String,
    pub new_file_uri: Uri,
}

pub fn document_edit(range: Range, text: String, document_uri: Uri) -> WorkspaceEdit {
    let text_edit = TextEdit { new_text: text, range };
    let mut changes = HashMap::new();
    changes.insert(document_uri, vec![text_edit]);
    WorkspaceEdit { changes: Some(changes), document_changes: None, change_annotations: None }
}

pub fn create_file(new_file_uri: Uri) -> WorkspaceEdit {
    WorkspaceEdit {
        changes: None,
        document_changes: Some(DocumentChanges::Operations(vec![
            DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                uri: new_file_uri,
                options: None,
                annotation_id: None,
            })),
        ])),
        change_annotations: None,
    }
}

/// The `lsp_types` document-change union, re-exported so callers do not need to
/// name the nested types.
pub use lsp_types::{DocumentChangeOperation, DocumentChanges};

fn line_is_empty(doc: &Doc, line: u32) -> bool {
    doc.text().line_content(line as usize).trim().is_empty()
}

pub fn table_of_contents_inner(include_levels: &[i32], doc: &Doc) -> Option<DocumentAction> {
    let toc = TableOfContents::mk(include_levels, doc.index())?;
    let rendered = toc.render();
    let existing_range = TableOfContents::detect(doc.text());

    let is_same = existing_range
        .map(|range| doc.text().substring(range))
        .map(|existing| TableOfContents::is_same(&rendered, &existing))
        .unwrap_or(false);

    if is_same {
        return None;
    }

    let name = match existing_range {
        None => "Create a Table of Contents",
        Some(_) => "Update the Table of Contents",
    };

    let insertion_point = match existing_range {
        Some(range) => InsertionPoint::Replacing(range),
        None => TableOfContents::insertion_point(doc),
    };

    log::trace!(
        "Determining table of contents insertion point: insertionPoint={insertion_point:?}, existing={existing_range:?}"
    );

    let empty_line = "\n\n";
    let line_break = "\n";

    let (edit_range, new_lines_before, new_lines_after) = match insertion_point {
        InsertionPoint::DocumentBeginning => {
            let after = if line_is_empty(doc, DOCUMENT_BEGINNING.start.line) {
                ""
            } else {
                empty_line
            };
            (DOCUMENT_BEGINNING, "", after)
        }
        InsertionPoint::Replacing(range) => {
            let before = if range.start.line == 0 || line_is_empty(doc, range.start.line - 1) {
                ""
            } else {
                empty_line
            };
            let after = if line_is_empty(doc, range.end.line + 1) { "" } else { empty_line };
            (range, before, after)
        }
        InsertionPoint::After(range) => {
            let line_after_last = range.end.line + 1;
            let new_range = Range::new(
                lsp_types::Position::new(line_after_last, 0),
                lsp_types::Position::new(line_after_last, 0),
            );
            let before = if line_is_empty(doc, range.end.line) { "" } else { line_break };
            let after = if line_is_empty(doc, line_after_last) { line_break } else { empty_line };
            (new_range, before, after)
        }
    };

    let text = format!("{new_lines_before}{rendered}{new_lines_after}");
    Some(DocumentAction { name: name.to_string(), new_text: text, edit: edit_range })
}

pub fn table_of_contents(config: &Config, doc: &Doc) -> Option<DocumentAction> {
    let include_levels = config.ca_toc_include();
    table_of_contents_inner(&include_levels, doc)
}

/// Offers to create the file a cross-document link points at, but only when no
/// document matches: a missing section still belongs to an existing file.
pub fn create_missing_file(range: Range, doc: &Doc, folder: &Folder) -> Option<CreateFileAction> {
    let configured_exts = folder.configured_markdown_exts();
    let pos = range.start;

    let at_pos = doc.index().link_at_pos(pos)?;
    let sym_at_pos = doc.structure().try_find_symbol_for_concrete(&at_pos)?;
    let doc_at_pos = match sym_at_pos {
        Sym::Ref(SymRef::CrossRef(r)) => r.doc().to_string(),
        _ => return None,
    };

    let matching_documents =
        folder.filter_docs_by_name(&InternName::mk_unchecked(doc.id().clone(), doc_at_pos.clone()));
    if !matching_documents.is_empty() {
        return None;
    }

    let intern_path = InternName::mk_unchecked(doc.id().clone(), doc_at_pos).try_as_path()?;

    let rel_path = RelPath::of_string_unchecked(ensure_markdown_ext(
        &configured_exts,
        intern_path.to_rel().to_system(),
    ));
    let path = folder.root_path().append(&rel_path);
    let filename = path.filename().to_string();
    let uri = path.to_uri();

    Some(CreateFileAction {
        name: format!("Create `{filename}`"),
        new_file_uri: crate::uri::parse(&uri),
    })
}

pub fn abs_path_to_uri(path: &AbsPath) -> String {
    path.to_uri()
}

pub fn is_markdown(exts: &[String], path: &str) -> bool {
    misc::is_markdown_file(exts, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ParserSettings;
    use crate::names::{DocId, FolderId};
    use crate::paths::LocalPath;
    use crate::text::mk_text;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-ca-{name}"));
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

    fn doc_of(content: &str) -> Doc {
        Doc::mk(
            &ParserSettings::default(),
            DocId::mk_rooted(&FolderId::of_uri("file:///w"), LocalPath::of_system("/w/a.md")),
            Some(1),
            mk_text(content),
        )
        .unwrap()
    }

    #[test]
    fn creates_a_toc_after_the_single_title() {
        let doc = doc_of("# T1\n## T2\n");
        let action = table_of_contents(&Config::default_config(), &doc).unwrap();
        assert_eq!(action.name, "Create a Table of Contents");
        assert_eq!(action.edit.start.line, 1);
        assert!(action.new_text.contains("<!--toc:start-->"));
        assert!(action.new_text.contains("- [T1](#t1)"));
        assert!(action.new_text.contains("  - [T2](#t2)"));
    }

    #[test]
    fn no_toc_action_without_headings() {
        assert!(table_of_contents(&Config::default_config(), &doc_of("text only\n")).is_none());
    }

    #[test]
    fn existing_identical_toc_produces_no_action() {
        let doc = doc_of("# T1\n\n<!--toc:start-->\n- [T1](#t1)\n<!--toc:end-->\n\n## T2\n");
        // The TOC is missing T2, so an update is still offered.
        assert_eq!(
            table_of_contents(&Config::default_config(), &doc).unwrap().name,
            "Update the Table of Contents"
        );

        let up_to_date = doc_of("# T1\n\n<!--toc:start-->\n- [T1](#t1)\n<!--toc:end-->\n");
        assert!(table_of_contents(&Config::default_config(), &up_to_date).is_none());
    }

    #[test]
    fn update_action_replaces_the_marked_range() {
        let doc = doc_of("# T1\n\n<!--toc:start-->\n- [old](#old)\n<!--toc:end-->\n\nbody\n");
        let action = table_of_contents(&Config::default_config(), &doc).unwrap();
        assert_eq!(action.edit.start.line, 2);
        assert_eq!(action.edit.end.line, 4);
        assert!(action.new_text.contains("- [T1](#t1)"));
    }

    #[test]
    fn toc_inserted_at_document_beginning_without_a_title() {
        let doc = doc_of("## T1\n## T2\n");
        let action = table_of_contents(&Config::default_config(), &doc).unwrap();
        assert_eq!(action.edit, DOCUMENT_BEGINNING);
        assert!(action.new_text.ends_with("\n\n"));
    }

    #[test]
    fn toc_inserted_after_front_matter() {
        let doc = doc_of("---\ntitle: X\n---\n\n## T1\n");
        let action = table_of_contents(&Config::default_config(), &doc).unwrap();
        assert!(action.edit.start.line >= 2);
    }

    #[test]
    fn respects_configured_heading_levels() {
        let config = Config { ca_toc_include: Some(vec![1, 2]), ..Config::empty() };
        let doc = doc_of("# T1\n## T2\n### T3\n");
        let action = table_of_contents(&config, &doc).unwrap();
        assert!(action.new_text.contains("T2"));
        assert!(!action.new_text.contains("T3"));
    }

    #[test]
    fn offers_to_create_a_missing_linked_file() {
        let dir = temp_dir("create");
        write(&dir, "a.md", "# A\n\n[[New Note]]\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let link_range = doc.index().wiki_links[0].range;
        let action =
            create_missing_file(Range::new(link_range.start, link_range.start), &doc, &folder)
                .unwrap();
        assert_eq!(action.name, "Create `New Note.md`");
        assert!(
            action.new_file_uri.as_str().ends_with("/New%20Note.md"),
            "unexpected uri: {}",
            action.new_file_uri.as_str()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_create_action_when_the_file_exists() {
        let dir = temp_dir("create-exists");
        write(&dir, "a.md", "# A\n\n[[B]]\n");
        write(&dir, "b.md", "# B\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let link_range = doc.index().wiki_links[0].range;
        assert!(create_missing_file(Range::new(link_range.start, link_range.start), &doc, &folder)
            .is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_create_action_for_a_missing_section_in_an_existing_file() {
        let dir = temp_dir("create-section");
        write(&dir, "a.md", "# A\n\n[[B#nope]]\n");
        write(&dir, "b.md", "# B\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "A");

        let link_range = doc.index().wiki_links[0].range;
        assert!(create_missing_file(Range::new(link_range.start, link_range.start), &doc, &folder)
            .is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn create_action_uses_a_document_change() {
        let edit = create_file(crate::uri::parse("file:///w/new.md"));
        assert!(edit.changes.is_none());
        match edit.document_changes {
            Some(DocumentChanges::Operations(ops)) => match &ops[0] {
                DocumentChangeOperation::Op(ResourceOp::Create(created)) => {
                    assert_eq!(created.uri.as_str(), "file:///w/new.md")
                }
                other => panic!("expected a create operation, got {other:?}"),
            },
            other => panic!("expected document change operations, got {other:?}"),
        }
    }

    #[test]
    fn document_edit_uses_plain_changes() {
        let edit = document_edit(
            Range::default(),
            "text".to_string(),
            crate::uri::parse("file:///w/a.md"),
        );
        assert!(edit.document_changes.is_none());
        assert_eq!(edit.changes.unwrap().len(), 1);
    }
}
