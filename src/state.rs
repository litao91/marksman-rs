//! Server state: the client description and the workspace, plus a revision counter.
//!
//! Port of `Marksman.State`.

use lsp_types::{ClientCapabilities, ClientInfo, InitializeParams, TextDocumentSyncKind};
use serde_json::Value;

use crate::config::{Config, TextSync};
use crate::doc::Doc;
use crate::folder::Folder;
use crate::names::FolderId;
use crate::paths::{AbsPath, UriWith};
use crate::workspace::Workspace;

#[derive(Clone, Debug, Default)]
pub struct InitOptions {
    pub preferred_text_sync_kind: Option<TextSync>,
}

impl InitOptions {
    pub fn of_json(json: &Value) -> InitOptions {
        match json.get("preferredTextSyncKind") {
            None => InitOptions::default(),
            Some(kind_num) => {
                // 1 and 2 are the protocol's Full and Incremental sync kinds.
                let kind = match kind_num.as_i64() {
                    Some(1) => Some(TextSync::Full),
                    Some(2) => Some(TextSync::Incremental),
                    _ => None,
                };
                let _ = TextDocumentSyncKind::FULL;
                InitOptions { preferred_text_sync_kind: kind }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ClientDescription {
    pub info: Option<ClientInfo>,
    pub caps: ClientCapabilities,
    pub opts: InitOptions,
}

fn client_name_is(client: &ClientDescription, name: &str) -> bool {
    client.info.as_ref().is_some_and(|x| x.name == name)
}

impl ClientDescription {
    pub fn is_vscode(&self) -> bool {
        client_name_is(self, "Visual Studio Code")
    }

    pub fn is_emacs(&self) -> bool {
        client_name_is(self, "emacs")
    }

    pub fn supports_document_edit(&self) -> bool {
        self.caps
            .workspace
            .as_ref()
            .and_then(|ws| ws.workspace_edit.as_ref())
            .and_then(|edit| edit.document_changes)
            == Some(true)
    }

    fn experimental_flag(&self, name: &str) -> bool {
        self.caps
            .experimental
            .as_ref()
            .and_then(|exp| exp.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub fn supports_status(&self) -> bool {
        self.experimental_flag("statusNotification")
    }

    pub fn supports_lens_find_references(&self) -> bool {
        self.experimental_flag("codeLensFindReferences")
    }

    pub fn supports_hierarchy(&self) -> bool {
        self.caps
            .text_document
            .as_ref()
            .and_then(|td| td.document_symbol.as_ref())
            .and_then(|ds| ds.hierarchical_document_symbol_support)
            .unwrap_or(false)
    }

    pub fn supports_prepare_rename(&self) -> bool {
        self.caps
            .text_document
            .as_ref()
            .and_then(|td| td.rename.as_ref())
            .and_then(|r| r.prepare_support)
            .unwrap_or(false)
    }

    pub fn preferred_text_sync_kind(&self) -> Option<TextSync> {
        self.opts.preferred_text_sync_kind
    }

    pub fn empty() -> ClientDescription {
        ClientDescription {
            info: None,
            caps: ClientCapabilities::default(),
            opts: InitOptions::default(),
        }
    }

    pub fn of_params(par: &InitializeParams) -> ClientDescription {
        let caps = par.capabilities.clone();
        let opts = par
            .initialization_options
            .as_ref()
            .map(InitOptions::of_json)
            .unwrap_or_default();

        ClientDescription { info: par.client_info.clone(), caps, opts }
    }
}

#[derive(Clone, Debug)]
pub struct State {
    client: ClientDescription,
    workspace: Workspace,
    revision: i64,
}

impl State {
    pub fn mk(client: ClientDescription, ws: Workspace) -> State {
        State { client, workspace: ws, revision: 0 }
    }

    pub fn client(&self) -> &ClientDescription {
        &self.client
    }

    pub fn client_caps(&self) -> &ClientCapabilities {
        &self.client.caps
    }

    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    pub fn user_config_or_default(&self) -> Config {
        self.workspace.user_config().unwrap_or_default()
    }

    pub fn revision(&self) -> i64 {
        self.revision
    }

    pub fn try_find_folder_enclosing(&self, uri: &UriWith<AbsPath>) -> Option<&Folder> {
        self.workspace.try_find_folder_enclosing(&uri.data)
    }

    pub fn find_folder_enclosing(&self, uri: &UriWith<AbsPath>) -> Folder {
        self.try_find_folder_enclosing(uri)
            .unwrap_or_else(|| panic!("Expected folder not found: {}", uri.uri))
            .clone()
    }

    pub fn try_find_folder_and_doc(&self, uri: &UriWith<AbsPath>) -> Option<(Folder, Doc)> {
        let folder = self.try_find_folder_enclosing(uri)?;
        let doc = folder.try_find_doc_by_path(&uri.data)?;
        Some((folder.clone(), doc))
    }

    pub fn try_find_doc(&self, uri: &UriWith<AbsPath>) -> Option<Doc> {
        self.try_find_folder_and_doc(uri).map(|(_, doc)| doc)
    }

    pub fn update_folders_from_lsp(
        &self,
        added: &[lsp_types::WorkspaceFolder],
        removed: &[lsp_types::WorkspaceFolder],
    ) -> State {
        log::trace!(
            "Updating workspace folders: numAdded={}, numRemoved={}",
            added.len(),
            removed.len()
        );

        let removed_uris: Vec<FolderId> =
            removed.iter().map(|f| FolderId::of_uri(f.uri.to_string())).collect();

        let user_config = self.workspace.user_config();

        let mut added_folders = Vec::new();
        for f in added {
            let root_uri = FolderId::of_uri(f.uri.to_string());
            let folder = if crate::folder::check_workspace_folder_with_warn(&root_uri) {
                Folder::try_load(user_config.clone(), &f.name, root_uri)
            } else {
                None
            };
            if let Some(folder) = folder {
                added_folders.push(folder);
            }
        }

        let new_workspace = self
            .workspace
            .without_folders(&removed_uris)
            .with_folders(added_folders);

        State {
            client: self.client.clone(),
            workspace: new_workspace,
            revision: self.revision + 1,
        }
    }

    pub fn update_folder(&self, new_folder: Folder) -> State {
        State {
            workspace: self.workspace.with_folder(new_folder),
            revision: self.revision + 1,
            ..self.clone()
        }
    }

    pub fn remove_folder(&self, key_path: &FolderId) -> State {
        State {
            workspace: self.workspace.without_folder(key_path),
            revision: self.revision + 1,
            ..self.clone()
        }
    }

    pub fn with_workspace(&self, workspace: Workspace) -> State {
        State { workspace, ..self.clone() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn init_options_reads_the_preferred_sync_kind() {
        let opts = InitOptions::of_json(&json!({ "preferredTextSyncKind": 2 }));
        assert_eq!(opts.preferred_text_sync_kind, Some(TextSync::Incremental));

        let opts = InitOptions::of_json(&json!({ "preferredTextSyncKind": 1 }));
        assert_eq!(opts.preferred_text_sync_kind, Some(TextSync::Full));

        assert_eq!(InitOptions::of_json(&json!({})).preferred_text_sync_kind, None);
        assert_eq!(
            InitOptions::of_json(&json!({ "preferredTextSyncKind": 99 })).preferred_text_sync_kind,
            None
        );
    }

    #[test]
    fn client_flags_come_from_experimental_capabilities() {
        let mut client = ClientDescription::empty();
        assert!(!client.supports_status());

        client.caps.experimental = Some(json!({
            "statusNotification": true,
            "codeLensFindReferences": true
        }));
        assert!(client.supports_status());
        assert!(client.supports_lens_find_references());
    }

    #[test]
    fn editor_detection_uses_the_client_name() {
        let mut client = ClientDescription::empty();
        client.info = Some(ClientInfo { name: "emacs".into(), version: None });
        assert!(client.is_emacs());
        assert!(!client.is_vscode());

        client.info = Some(ClientInfo { name: "Visual Studio Code".into(), version: None });
        assert!(client.is_vscode());
        assert!(!client.is_emacs());
    }

    #[test]
    fn hierarchy_and_prepare_rename_support_are_read_from_capabilities() {
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                text_document: Some(lsp_types::TextDocumentClientCapabilities {
                    document_symbol: Some(lsp_types::DocumentSymbolClientCapabilities {
                        hierarchical_document_symbol_support: Some(true),
                        ..Default::default()
                    }),
                    rename: Some(lsp_types::RenameClientCapabilities {
                        prepare_support: Some(true),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let client = ClientDescription::of_params(&params);
        assert!(client.supports_hierarchy());
        assert!(client.supports_prepare_rename());
        assert!(!client.supports_document_edit());
    }

    #[test]
    fn state_revision_grows_with_folder_updates() {
        let state = State::mk(ClientDescription::empty(), Workspace::default());
        assert_eq!(state.revision(), 0);
        assert_eq!(state.remove_folder(&FolderId::of_uri("file:///x")).revision(), 1);
    }

    #[test]
    fn user_config_defaults_when_unset() {
        let state = State::mk(ClientDescription::empty(), Workspace::default());
        assert_eq!(state.user_config_or_default().compl_candidates(), 50);
    }
}
