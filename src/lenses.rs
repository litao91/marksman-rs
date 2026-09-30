//! Code lenses showing how many references point at a heading or link definition.
//!
//! Port of `Marksman.Lenses`.

use lsp_types::{CodeLens, Command, Location, Position, Uri};
use serde::{Deserialize, Serialize};

use crate::cst::Element;
use crate::doc::Doc;
use crate::folder::Folder;
use crate::refs::Dest;
use crate::state::ClientDescription;

pub const FIND_REFERENCES_LENS: &str = "marksman.findReferences";

/// Payload handed back to the client so it can show the references directly.
/// Field names are camelCase to match the original's JSON contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FindReferencesData {
    pub uri: Uri,
    pub position: Position,
    pub locations: Vec<Location>,
}

fn human_ref_count(cnt: usize) -> String {
    if cnt == 1 {
        "1 reference".to_string()
    } else {
        format!("{cnt} references")
    }
}

fn url_of(uri: &str) -> Uri {
    crate::uri::parse(uri)
}

pub fn build_reference_lens(
    client: &ClientDescription,
    folder: &Folder,
    doc: &Doc,
    el: &Element,
) -> Option<CodeLens> {
    let refs: Vec<(Doc, Element)> = Dest::find_element_refs(false, folder, doc, el);
    let ref_count = refs.len();

    if ref_count == 0 {
        return None;
    }

    let data = if client.supports_lens_find_references() {
        let locations: Vec<Location> = refs
            .iter()
            .map(|(doc, el)| Location { uri: url_of(doc.uri()), range: el.range() })
            .collect();

        let payload = FindReferencesData {
            uri: url_of(doc.uri()),
            position: el.range().start,
            locations,
        };

        Some(vec![serde_json::to_value(payload).ok()?])
    } else {
        None
    };

    Some(CodeLens {
        range: el.range(),
        command: Some(Command {
            title: human_ref_count(ref_count),
            command: FIND_REFERENCES_LENS.to_string(),
            arguments: data,
        }),
        data: None,
    })
}

pub fn for_doc(client: &ClientDescription, folder: &Folder, doc: &Doc) -> Vec<CodeLens> {
    let heading_lenses = doc
        .index()
        .headings
        .iter()
        .map(|h| Element::H(h.clone()))
        .filter_map(|el| build_reference_lens(client, folder, doc, &el));

    let link_def_lenses = doc
        .index()
        .link_defs
        .iter()
        .map(|d| Element::MLD(d.clone()))
        .filter_map(|el| build_reference_lens(client, folder, doc, &el));

    heading_lenses.chain(link_def_lenses).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::names::FolderId;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-lens-{name}"));
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
    fn lens_counts_incoming_references() {
        let dir = temp_dir("count");
        write(&dir, "a.md", "# A\n\n[[Target]]\n");
        write(&dir, "b.md", "# B\n\n[[Target]]\n");
        write(&dir, "target.md", "# Target\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "Target");

        let lenses = for_doc(&ClientDescription::empty(), &folder, &doc);
        assert_eq!(lenses.len(), 1);
        let command = lenses[0].command.as_ref().unwrap();
        assert_eq!(command.title, "2 references");
        assert_eq!(command.command, FIND_REFERENCES_LENS);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn singular_reference_is_worded_differently() {
        let dir = temp_dir("singular");
        write(&dir, "a.md", "# A\n\n[[Target]]\n");
        write(&dir, "target.md", "# Target\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "Target");

        let lenses = for_doc(&ClientDescription::empty(), &folder, &doc);
        assert_eq!(lenses[0].command.as_ref().unwrap().title, "1 reference");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unreferenced_headings_get_no_lens() {
        let dir = temp_dir("unreferenced");
        write(&dir, "a.md", "# A\n\n## Lonely\n");
        let folder = load(&dir);
        assert!(for_doc(&ClientDescription::empty(), &folder, &doc_named(&folder, "A")).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn link_definitions_get_lenses_too() {
        let dir = temp_dir("linkdef-lens");
        write(&dir, "a.md", "See [ref].\n\n[ref]: /url\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "a");

        let lenses = for_doc(&ClientDescription::empty(), &folder, &doc);
        assert_eq!(lenses.len(), 1);
        assert_eq!(lenses[0].command.as_ref().unwrap().title, "1 reference");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn locations_are_attached_only_when_the_client_supports_them() {
        let dir = temp_dir("locations");
        write(&dir, "a.md", "# A\n\n[[Target]]\n");
        write(&dir, "target.md", "# Target\n");
        let folder = load(&dir);
        let doc = doc_named(&folder, "Target");

        let plain = for_doc(&ClientDescription::empty(), &folder, &doc);
        assert!(plain[0].command.as_ref().unwrap().arguments.is_none());

        let mut capable = ClientDescription::empty();
        capable.caps.experimental = Some(serde_json::json!({ "codeLensFindReferences": true }));
        let with_data = for_doc(&capable, &folder, &doc);
        let args = with_data[0].command.as_ref().unwrap().arguments.as_ref().unwrap();
        assert_eq!(args.len(), 1);
        assert_eq!(args[0]["uri"].as_str(), Some(doc.uri()));
        assert!(args[0]["locations"].is_array());
        std::fs::remove_dir_all(&dir).ok();
    }
}
