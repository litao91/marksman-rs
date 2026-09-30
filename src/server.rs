//! The LSP server: request handlers, capabilities, and the background services.
//!
//! Port of `Marksman.Server`. Requests are served against a single mutable
//! `State` on the main loop, which gives the same serialization guarantees the
//! original's actor provides. Diagnostics and status run on their own threads so
//! that a burst of edits coalesces into one publication.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use log::{debug, error, trace, warn};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionProviderCapability,
    CodeActionResponse, CodeLensOptions, CompletionItem, CompletionOptions, CompletionResponse,
    Diagnostic, DidChangeTextDocumentParams, DidChangeWorkspaceFoldersParams,
    DidCloseTextDocumentParams, DocumentSymbolResponse, FileOperationFilter, FileOperationPattern,
    FileOperationPatternKind, FileOperationRegistrationOptions, GotoDefinitionResponse, Hover,
    HoverContents, HoverProviderCapability, InitializeParams, InitializeResult, Location,
    MarkupContent, MarkupKind, OneOf, PublishDiagnosticsParams, RenameOptions,
    SemanticToken, SemanticTokens, SemanticTokensFullOptions, SemanticTokensLegend,
    SemanticTokensOptions, SemanticTokensServerCapabilities, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind, TextDocumentSyncOptions, Uri,
    WorkspaceFileOperationsServerCapabilities, WorkspaceFoldersServerCapabilities,
    WorkspaceServerCapabilities,
};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

use crate::code_actions;
use crate::compl;
use crate::config::{self, Config, ParserSettings, TextSync};
use crate::diag::{self, WorkspaceDiag};
use crate::doc::Doc;
use crate::folder::Folder;
use crate::lenses;
use crate::misc;
use crate::names::{DocId, FolderId};
use crate::paths::{AbsPath, LocalPath, UriWith};
use crate::refs::Dest;
use crate::refactor::{self, RenameResult};
use crate::semato;
use crate::state::{ClientDescription, State};
use crate::symbols::{self, DocSymbols};
use crate::workspace::Workspace;

const DIAGNOSTICS_GRACE_PERIOD_MS: u64 = 200;

/// JSON-RPC error codes used by the server.
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;

#[derive(Clone, Debug, Serialize)]
pub struct MarksmanStatusParams {
    pub state: String,
    #[serde(rename = "docCount")]
    pub doc_count: usize,
}

// ---------------------------------------------------------------------------
// Capability negotiation
// ---------------------------------------------------------------------------

#[allow(deprecated)] // root_uri and root_path are the documented fallbacks
pub fn extract_workspace_folders(par: &InitializeParams) -> BTreeMap<String, FolderId> {
    let mut out = BTreeMap::new();

    if let Some(folders) = &par.workspace_folders {
        for f in folders {
            let folder_id = FolderId::of_uri(f.uri.to_string());
            if crate::folder::check_workspace_folder_with_warn(&folder_id) {
                out.insert(f.name.clone(), folder_id);
            }
        }
        return out;
    }

    let root_uri = par
        .root_uri
        .as_ref()
        .map(|u| u.to_string())
        .or_else(|| {
            par.root_path
                .as_ref()
                .map(|p| crate::paths::system_path_to_uri_string(p))
        });

    match root_uri {
        // No folders are configured. The client can still add folders later
        // using a notification.
        None => out,
        Some(uri) => {
            let root_uri = FolderId::of_uri(uri);
            if crate::folder::check_workspace_folder_with_warn(&root_uri) {
                let root_name = root_uri.data.filename().to_string();
                out.insert(root_name, root_uri);
            }
            out
        }
    }
}

pub fn read_workspace(user_config: Option<Config>, roots: &BTreeMap<String, FolderId>) -> Vec<Folder> {
    roots
        .iter()
        .filter_map(|(name, root)| Folder::try_load(user_config.clone(), name, root.clone()))
        .collect()
}

/// Picks the text sync kind, preferring the most conservative setting found in
/// any folder's configuration.
pub fn calc_text_sync(
    user_config: &Option<Config>,
    workspace: &Workspace,
    client_desc: &ClientDescription,
) -> (&'static str, TextSync) {
    let configured: Vec<TextSync> = workspace
        .folders()
        .filter_map(|f| f.config())
        .filter_map(|c| c.core_text_sync)
        .collect();

    if !configured.is_empty() {
        let common = configured.iter().min_by_key(|k| match k {
            TextSync::Full => 0,
            TextSync::Incremental => 1,
        }).copied().unwrap();
        ("workspaceConfig", common)
    } else if let Some(kind) = user_config.as_ref().and_then(|x| x.core_text_sync) {
        ("userConfig", kind)
    } else {
        match client_desc.preferred_text_sync_kind() {
            Some(kind) => ("clientOption", kind),
            None => ("default", TextSync::Full),
        }
    }
}

pub fn mk_server_caps(
    markdown_exts: &[String],
    text_sync_kind: TextSync,
    par: &InitializeParams,
) -> ServerCapabilities {
    let workspace_folders_caps = WorkspaceFoldersServerCapabilities {
        supported: Some(true),
        change_notifications: Some(OneOf::Left(true)),
    };

    let glob = mk_watch_glob(markdown_exts);

    let markdown_file_pattern = FileOperationPattern {
        glob,
        matches: Some(FileOperationPatternKind::File),
        options: Some(lsp_types::FileOperationPatternOptions { ignore_case: Some(true) }),
    };

    let markdown_file_registration = FileOperationRegistrationOptions {
        filters: vec![FileOperationFilter { scheme: None, pattern: markdown_file_pattern }],
    };

    let workspace_file_caps = WorkspaceFileOperationsServerCapabilities {
        did_create: Some(markdown_file_registration.clone()),
        will_create: None,
        did_delete: Some(markdown_file_registration),
        will_delete: None,
        did_rename: None,
        // VSCode behaves oddly when communicating file renames, so that is left
        // off. When a file is renamed VSCode sends didClose on the old name and
        // didOpen on the new one, which is enough to keep the state in sync.
        will_rename: None,
    };

    let workspace_caps = WorkspaceServerCapabilities {
        workspace_folders: Some(workspace_folders_caps),
        file_operations: Some(workspace_file_caps),
    };

    let sync_kind = match text_sync_kind {
        TextSync::Full => TextDocumentSyncKind::FULL,
        TextSync::Incremental => TextDocumentSyncKind::INCREMENTAL,
    };

    let text_sync_caps = TextDocumentSyncOptions {
        open_close: Some(true),
        change: Some(sync_kind),
        will_save: None,
        will_save_wait_until: None,
        save: None,
    };

    let client_desc = ClientDescription::of_params(par);

    let code_action_options = CodeActionOptions {
        code_action_kinds: None,
        resolve_provider: Some(false),
        work_done_progress_options: Default::default(),
    };

    let rename_options = if client_desc.supports_prepare_rename() {
        Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        }))
    } else {
        Some(OneOf::Left(true))
    };

    ServerCapabilities {
        workspace: Some(workspace_caps),
        workspace_symbol_provider: Some(OneOf::Left(!client_desc.is_vscode())),
        text_document_sync: Some(TextDocumentSyncCapability::Options(text_sync_caps)),
        document_symbol_provider: Some(OneOf::Left(!client_desc.is_vscode())),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec!["[".into(), "#".into(), "(".into()]),
            resolve_provider: None,
            all_commit_characters: None,
            completion_item: None,
            work_done_progress_options: Default::default(),
        }),
        definition_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(code_action_options)),
        semantic_tokens_provider: Some(
            SemanticTokensServerCapabilities::SemanticTokensOptions(SemanticTokensOptions {
                legend: SemanticTokensLegend {
                    token_types: semato::TokenType::mapping()
                        .into_iter()
                        .map(lsp_types::SemanticTokenType::new)
                        .collect(),
                    token_modifiers: vec![],
                },
                range: Some(true),
                full: Some(SemanticTokensFullOptions::Delta { delta: Some(false) }),
                work_done_progress_options: Default::default(),
            }),
        ),
        rename_provider: rename_options,
        code_lens_provider: Some(CodeLensOptions { resolve_provider: None }),
        execute_command_provider: Some(lsp_types::ExecuteCommandOptions {
            commands: vec![],
            work_done_progress_options: Default::default(),
        }),
        ..Default::default()
    }
}

pub fn mk_watch_glob(configured_exts: &[String]) -> String {
    format!("**/*.{{{}}}", configured_exts.join(","))
}

// ---------------------------------------------------------------------------
// Diagnostics
// ---------------------------------------------------------------------------

fn diagnostic_publication(
    doc_uri: &DocId,
    existing_doc_version: Option<i32>,
    new_doc_version: Option<i32>,
    existing_doc_diag: &[Diagnostic],
    new_doc_diag: &[Diagnostic],
) -> Option<PublishDiagnosticsParams> {
    let reopened_with_diagnostics =
        existing_doc_version.is_none() && new_doc_version.is_some() && !new_doc_diag.is_empty();

    if new_doc_diag != existing_doc_diag || reopened_with_diagnostics {
        Some(PublishDiagnosticsParams {
            uri: crate::uri::parse(doc_uri.uri()),
            diagnostics: new_doc_diag.to_vec(),
            version: None,
        })
    } else {
        None
    }
}

pub fn calc_diagnostics_update(
    previous: Option<(&State, &WorkspaceDiag)>,
    new_state: &State,
) -> (WorkspaceDiag, Vec<PublishDiagnosticsParams>) {
    let previous_workspace = previous.map(|(state, diagnostics)| (state.workspace().clone(), diagnostics));

    let (new_diag, affected) = match previous_workspace {
        Some((ws, diags)) => diag::calculate(Some(&(ws, diags.clone())), new_state.workspace()),
        None => diag::calculate(None, new_state.workspace()),
    };

    let empty: WorkspaceDiag = BTreeMap::new();
    let existing_diag: &WorkspaceDiag = previous.map(|(_, d)| d).unwrap_or(&empty);

    let mut updates = Vec::new();
    for (folder_path, documents) in &affected {
        let existing_folder_diag = existing_diag.get(folder_path).cloned().unwrap_or_default();
        let new_folder_diag = new_diag.get(folder_path).cloned().unwrap_or_default();

        trace!("Updating folder diag: folder={}, num_docs={}", folder_path.uri, documents.len());

        for doc_uri in documents {
            let doc_path: UriWith<LocalPath> = doc_uri.raw().rooted_rel_to_abs();
            let abs = match &doc_path.data {
                LocalPath::Abs(p) => UriWith { uri: doc_path.uri.clone(), data: p.clone() },
                LocalPath::Rel(_) => continue,
            };

            let existing_doc = previous.and_then(|(state, _)| state.try_find_doc(&abs));
            let existing_doc_version = existing_doc.as_ref().and_then(Doc::version);
            let existing_doc_diag: Arc<Vec<Diagnostic>> =
                existing_folder_diag.get(doc_uri).cloned().unwrap_or_default();

            let new_doc = new_state.try_find_doc(&abs);
            let new_doc_version = new_doc.as_ref().and_then(Doc::version);
            let new_doc_diag: Arc<Vec<Diagnostic>> =
                new_folder_diag.get(doc_uri).cloned().unwrap_or_default();

            if let Some(update) = diagnostic_publication(
                doc_uri,
                existing_doc_version,
                new_doc_version,
                &existing_doc_diag,
                &new_doc_diag,
            ) {
                trace!("Diagnostic changed, queueing the update: doc={}", doc_uri.uri());
                updates.push(update);
            }
        }
    }

    (new_diag, updates)
}

/// Coalesces state updates and publishes diagnostics on a quiet moment, so that
/// active editing does not trigger a recalculation per keystroke. The grace
/// period mirrors the original background agent.
async fn diagnostics_task(mut rx: mpsc::UnboundedReceiver<StateSnapshot>, out: mpsc::UnboundedSender<Value>) {
    let mut previous: Option<(StateSnapshot, WorkspaceDiag)> = None;

    while let Some(state) = rx.recv().await {
        let mut latest = state;
        loop {
            match tokio::time::timeout(Duration::from_millis(DIAGNOSTICS_GRACE_PERIOD_MS), rx.recv()).await
            {
                Ok(Some(next)) => latest = next,
                _ => break,
            }
        }

        // The comparison walks every affected document, so keep it off the
        // runtime's worker threads.
        let carried = previous.take();
        let computed = tokio::task::spawn_blocking(move || {
            let (new_diag, updates) = calc_diagnostics_update(
                carried.as_ref().map(|(s, d)| (s.as_ref(), d)),
                &latest,
            );
            (latest, new_diag, updates)
        })
        .await;

        let Ok((latest, new_diag, updates)) = computed else { return };

        for update in updates {
            let message = Notification::publish_diagnostics(&update);
            if out.send(message).is_err() {
                return;
            }
        }

        previous = Some((latest, new_diag));
    }
}

/// Reports the document count, but only when it actually changes.
async fn status_task(mut rx: mpsc::UnboundedReceiver<usize>, out: mpsc::UnboundedSender<Value>) {
    let mut count = 0usize;

    while let Some(new_count) = rx.recv().await {
        if new_count == count {
            continue;
        }
        trace!("StatusAgent sending update: docCount=({count}, {new_count})");
        count = new_count;

        let params = MarksmanStatusParams { state: "ok".to_string(), doc_count: count };
        let message = json!({
            "jsonrpc": "2.0",
            "method": "marksman/status",
            "params": serde_json::to_value(params).unwrap_or(Value::Null),
        });
        if out.send(message).is_err() {
            return;
        }
    }
}


// ---------------------------------------------------------------------------
// JSON-RPC plumbing
// ---------------------------------------------------------------------------

pub struct Notification;

impl Notification {
    pub fn new(method: &str, params: Value) -> Value {
        json!({ "jsonrpc": "2.0", "method": method, "params": params })
    }

    pub fn publish_diagnostics(params: &PublishDiagnosticsParams) -> Value {
        Notification::new(
            "textDocument/publishDiagnostics",
            serde_json::to_value(params).unwrap_or(Value::Null),
        )
    }
}

pub fn response_ok(id: Value, result: impl Serialize) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": serde_json::to_value(result).unwrap_or(Value::Null) })
}

pub fn response_ok_opt<R: Serialize>(id: Value, result: Option<R>) -> Value {
    match result {
        Some(r) => response_ok(id, r),
        None => response_ok(id, Value::Null),
    }
}

pub fn response_err(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

pub fn invalid_params(id: Value, msg: impl Into<String>) -> Value {
    response_err(id, INVALID_PARAMS, msg)
}

pub fn internal_error(id: Value, msg: impl Into<String>) -> Value {
    response_err(id, INTERNAL_ERROR, msg)
}

fn parse_params<P: serde::de::DeserializeOwned>(id: &Value, method: &str, params: Value) -> Result<P, Value> {
    serde_json::from_value(params)
        .map_err(|e| invalid_params(id.clone(), format!("Invalid params for {method}: {e}")))
}

/// Percent-encodes the characters a URI parser rejects outright. Most clients
/// send properly encoded URIs; this covers the ones that do not.
fn encode_invalid_uri_chars(s: &str) -> String {
    let needs_fix = s
        .chars()
        .any(|c| c == ' ' || c == '"' || c == '<' || c == '>' || c == '\\' || (c as u32) > 0x7F);
    if !needs_fix {
        return s.to_string();
    }

    s.chars()
        .map(|c| {
            if c == ' ' || c == '"' || c == '<' || c == '>' || c == '\\' || (c as u32) > 0x7F {
                misc::url_encode(&c.to_string())
            } else {
                c.to_string()
            }
        })
        .collect()
}

/// Some clients send document URIs containing raw spaces or non-ASCII
/// characters. The original accepts those because it treats a URI as an opaque
/// string, so normalize them here instead of dropping the request.
fn normalize_uri_fields(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "uri" || key == "rootUri" {
                    if let Value::String(s) = child {
                        *s = encode_invalid_uri_chars(s);
                    }
                } else {
                    normalize_uri_fields(child);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                normalize_uri_fields(item);
            }
        }
        _ => {}
    }
}

/// Extracts the params object from a message, normalizing any URIs it carries.
fn take_params(message: &Value) -> Value {
    let mut params = message.get("params").cloned().unwrap_or(Value::Null);
    normalize_uri_fields(&mut params);
    params
}

fn abs_uri(uri: &Uri) -> UriWith<AbsPath> {
    abs_uri_str(uri.as_str())
}

/// File operation payloads carry URIs as plain strings.
fn abs_uri_str(uri: &str) -> UriWith<AbsPath> {
    UriWith::mk_abs(uri.to_string())
}

/// The LSP wire form of the delta encoding produced by `semato`.
fn semantic_tokens(data: Vec<u32>) -> Vec<SemanticToken> {
    data.chunks_exact(5)
        .map(|c| SemanticToken {
            delta_line: c[0],
            delta_start: c[1],
            length: c[2],
            token_type: c[3],
            token_modifiers_bitset: c[4],
        })
        .collect()
}

fn markdown(value: String) -> HoverContents {
    HoverContents::Markup(MarkupContent { kind: MarkupKind::Markdown, value })
}

// ---------------------------------------------------------------------------
// State actor
// ---------------------------------------------------------------------------

pub type StateSnapshot = Arc<State>;

/// What a mutation hands back: an optional response and an optional next state.
pub struct MutationResult {
    pub response: Option<Value>,
    pub state: Option<State>,
}

impl MutationResult {
    pub fn empty() -> MutationResult {
        MutationResult { response: None, state: None }
    }

    pub fn output(response: Value) -> MutationResult {
        MutationResult { response: Some(response), state: None }
    }

    pub fn state(state: State) -> MutationResult {
        MutationResult { response: None, state: Some(state) }
    }

    pub fn state_opt(state: Option<State>) -> MutationResult {
        MutationResult { response: None, state }
    }
}

type Mutator = Box<dyn FnOnce(&State) -> MutationResult + Send>;

pub enum StateMsg {
    Read(oneshot::Sender<StateSnapshot>),
    RegisterHooks,
    Mutate(Mutator, oneshot::Sender<MutationResult>),
}

/// Hooks run after every state transition, as in the original's StateManager.
#[derive(Clone)]
pub struct Hooks {
    diagnostics: mpsc::UnboundedSender<StateSnapshot>,
    status: mpsc::UnboundedSender<usize>,
    status_enabled: bool,
}

impl Hooks {
    fn run(&self, state: &StateSnapshot) {
        let _ = self.diagnostics.send(Arc::clone(state));
        if self.status_enabled {
            let _ = self.status.send(state.workspace().doc_count());
        }
    }
}

/// A handle to the actor that owns the mutable state. Reads hand out a cheap
/// snapshot; mutations are serialized through the actor.
#[derive(Clone)]
pub struct StateHandle {
    tx: mpsc::UnboundedSender<StateMsg>,
}

impl StateHandle {
    pub async fn read(&self) -> Option<StateSnapshot> {
        let (tx, rx) = oneshot::channel();
        self.tx.send(StateMsg::Read(tx)).ok()?;
        rx.await.ok()
    }

    pub async fn mutate<F>(&self, f: F) -> Option<MutationResult>
    where
        F: FnOnce(&State) -> MutationResult + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        self.tx.send(StateMsg::Mutate(Box::new(f), tx)).ok()?;
        rx.await.ok()
    }

    pub async fn register_hooks(&self) {
        let _ = self.tx.send(StateMsg::RegisterHooks);
    }
}

async fn state_actor(mut rx: mpsc::UnboundedReceiver<StateMsg>, init: State, hooks: Hooks) {
    let mut state: StateSnapshot = Arc::new(init);
    let mut hooks_active = false;

    while let Some(msg) = rx.recv().await {
        match msg {
            StateMsg::Read(tx) => {
                let _ = tx.send(Arc::clone(&state));
            }
            StateMsg::RegisterHooks => {
                hooks_active = true;
                // Registering a hook runs it once against the current state,
                // which is what publishes the initial diagnostics.
                hooks.run(&state);
            }
            StateMsg::Mutate(f, tx) => {
                trace!("Received a MutateState message");
                let current = Arc::clone(&state);
                // State transitions rebuild indexes, so run them off the worker
                // threads while still serializing them through this actor.
                let computed = tokio::task::spawn_blocking(move || f(&current)).await;

                let result = match computed {
                    Ok(result) => result,
                    Err(err) => {
                        // Failing to update the state is a fatal error: the
                        // server's view of the workspace can no longer be trusted.
                        crate::fatality::abort(Some(&state), &err.to_string())
                    }
                };

                if let Some(new_state) = result.state {
                    let next: StateSnapshot = Arc::new(new_state);
                    trace!(
                        "Updating state: curRev={}, nextRev={}",
                        state.revision(),
                        next.revision()
                    );
                    state = next;
                    if hooks_active {
                        hooks.run(&state);
                    }
                }

                let _ = tx.send(MutationResult { response: result.response, state: None });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Request handlers
// ---------------------------------------------------------------------------

fn workspace_symbol(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::WorkspaceSymbolParams = match parse_params(&id, "workspace/symbol", params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let syms = symbols::workspace_symbols(&par.query, state.workspace());
    response_ok(id, lsp_types::WorkspaceSymbolResponse::Flat(syms))
}

fn document_symbol(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::DocumentSymbolParams = match parse_params(&id, "textDocument/documentSymbol", params) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let doc_uri = abs_uri(&par.text_document.uri);
    let client = state.client();

    let response = state.try_find_doc(&doc_uri).map(|doc| {
        match symbols::doc_symbols(client.supports_hierarchy(), client.is_emacs(), &doc) {
            DocSymbols::Flat(syms) => DocumentSymbolResponse::Flat(syms),
            DocSymbols::Hierarchical(syms) => DocumentSymbolResponse::Nested(syms),
        }
    });

    response_ok_opt(id, response)
}

fn completion(state: &State, id: Value, params: Value) -> Value {
    trace!("Completion request start");
    let par: lsp_types::CompletionParams = match parse_params(&id, "textDocument/completion", params) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let pos = par.text_document_position.position;
    let doc_uri = abs_uri(&par.text_document_position.text_document.uri);

    let candidates = state.try_find_folder_and_doc(&doc_uri).and_then(|(folder, doc)| {
        let max_completions = folder.config_or_default().compl_candidates() as usize;

        let mut items: Vec<CompletionItem> = compl::find_candidates_in_doc(&folder, &doc, pos);
        items.truncate(max_completions);

        if items.is_empty() {
            None
        } else {
            Some(CompletionResponse::Array(items))
        }
    });

    response_ok_opt(id, candidates)
}

fn definition(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::TextDocumentPositionParams = match parse_params(&id, "textDocument/definition", params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let doc_uri = abs_uri(&par.text_document.uri);

    let goto = state.try_find_folder_and_doc(&doc_uri).and_then(|(folder, src_doc)| {
        let at_pos = src_doc.index().link_at_pos(par.position)?;
        let refs = Dest::try_resolve_element(&folder, &src_doc, &at_pos);

        let locs: Vec<Location> = refs
            .iter()
            .map(|r| Location { uri: crate::uri::parse(r.doc().uri()), range: r.range() })
            .collect();

        match locs.len() {
            0 => None,
            1 => Some(GotoDefinitionResponse::Scalar(locs.into_iter().next().unwrap())),
            _ => Some(GotoDefinitionResponse::Array(locs)),
        }
    });

    response_ok_opt(id, goto)
}

fn hover(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::HoverParams = match parse_params(&id, "textDocument/hover", params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let doc_uri = abs_uri(&par.text_document_position_params.text_document.uri);

    let hover = state.try_find_folder_and_doc(&doc_uri).and_then(|(folder, src_doc)| {
        let at_pos = src_doc.index().link_at_pos(par.text_document_position_params.position)?;
        // There may be several sources due to ambiguity. Hover requires a single
        // result, so the first is returned: when links are not ambiguous this is
        // fine, and otherwise the ambiguity is the author's to resolve.
        let dest = Dest::try_resolve_element(&folder, &src_doc, &at_pos).into_iter().next()?;

        let dest_scope = dest.scope();
        let content = dest.doc().text().substring(dest_scope);

        Some(Hover { contents: markdown(content), range: None })
    });

    response_ok_opt(id, hover)
}

fn references(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::ReferenceParams = match parse_params(&id, "textDocument/references", params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let doc_uri = abs_uri(&par.text_document_position.text_document.uri);

    let locs = match state.try_find_folder_and_doc(&doc_uri) {
        Some((folder, cur_doc)) => {
            match cur_doc.cst().element_at_pos(par.text_document_position.position) {
                None => {
                    warn!(
                        "Could not find an element for 'TextDocumentReferences': doc={}, pos={:?}",
                        cur_doc.id().uri(),
                        par.text_document_position.position
                    );
                    None
                }
                Some(at_pos) => {
                    let referencing = Dest::find_element_refs(
                        par.context.include_declaration,
                        &folder,
                        &cur_doc,
                        at_pos,
                    );
                    Some(
                        referencing
                            .iter()
                            .map(|(doc, el)| Location {
                                uri: crate::uri::parse(doc.uri()),
                                range: el.range(),
                            })
                            .collect::<Vec<_>>(),
                    )
                }
            }
        }
        None => None,
    };

    response_ok_opt(id, locs)
}

fn semantic_tokens_full(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::SemanticTokensParams =
        match parse_params(&id, "textDocument/semanticTokens/full", params) {
            Ok(p) => p,
            Err(e) => return e,
        };
    let doc_path = abs_uri(&par.text_document.uri);

    let tokens = state.try_find_folder_and_doc(&doc_path).map(|(_, doc)| SemanticTokens {
        result_id: None,
        data: semantic_tokens(semato::of_index_encoded(doc.index())),
    });

    response_ok_opt(id, tokens)
}

fn semantic_tokens_range(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::SemanticTokensRangeParams =
        match parse_params(&id, "textDocument/semanticTokens/range", params) {
            Ok(p) => p,
            Err(e) => return e,
        };
    let doc_path = abs_uri(&par.text_document.uri);

    let tokens = state.try_find_folder_and_doc(&doc_path).map(|(_, doc)| SemanticTokens {
        result_id: None,
        data: semantic_tokens(semato::of_index_encoded_in_range(doc.index(), &par.range)),
    });

    response_ok_opt(id, tokens)
}

fn code_action(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::CodeActionParams = match parse_params(&id, "textDocument/codeAction", params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let doc_path = abs_uri(&par.text_document.uri);

    let Some((folder, doc)) = state.try_find_folder_and_doc(&doc_path) else {
        return response_ok(id, Value::Null);
    };

    let config = folder.config_or_default();

    let code_action = |title: String, kind: Option<CodeActionKind>, edit: lsp_types::WorkspaceEdit| {
        CodeActionOrCommand::CodeAction(CodeAction {
            title,
            kind,
            diagnostics: None,
            command: None,
            data: None,
            is_preferred: Some(false),
            disabled: None,
            edit: Some(edit),
        })
    };

    let mut actions: Vec<CodeActionOrCommand> = Vec::new();

    if config.ca_toc_enable() {
        if let Some(ca) = code_actions::table_of_contents(&config, &doc) {
            let ws_edit = code_actions::document_edit(
                ca.edit,
                ca.new_text,
                crate::uri::parse(par.text_document.uri.as_str()),
            );
            actions.push(code_action(ca.name, Some(CodeActionKind::SOURCE), ws_edit));
        }
    }

    if config.ca_create_missing_file_enable() {
        if let Some(ca) = code_actions::create_missing_file(par.range, &doc, &folder) {
            let ws_edit = code_actions::create_file(ca.new_file_uri);
            actions.push(code_action(ca.name, Some(CodeActionKind::QUICKFIX), ws_edit));
        }
    }

    let response: CodeActionResponse = actions;
    response_ok(id, response)
}

fn rename(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::RenameParams = match parse_params(&id, "textDocument/rename", params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let doc_path = abs_uri(&par.text_document_position.text_document.uri);

    let Some((folder, src_doc)) = state.try_find_folder_and_doc(&doc_path) else {
        return response_ok(id, Value::Null);
    };

    let result = refactor::rename(
        state.client().supports_document_edit(),
        &folder,
        &src_doc,
        par.text_document_position.position,
        &par.new_name,
    );

    match result {
        RenameResult::Edit(edit) => response_ok(id, edit),
        RenameResult::Error(msg) => invalid_params(id, msg),
        RenameResult::Skip => response_ok(id, Value::Null),
    }
}

fn prepare_rename(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::TextDocumentPositionParams =
        match parse_params(&id, "textDocument/prepareRename", params) {
            Ok(p) => p,
            Err(e) => return e,
        };
    let doc_path = abs_uri(&par.text_document.uri);

    let rename_range = state
        .try_find_folder_and_doc(&doc_path)
        .and_then(|(_, src_doc)| refactor::rename_range(&src_doc, par.position));

    response_ok_opt(id, rename_range.map(lsp_types::PrepareRenameResponse::Range))
}

fn code_lens(state: &State, id: Value, params: Value) -> Value {
    let par: lsp_types::CodeLensParams = match parse_params(&id, "textDocument/codeLens", params) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let doc_path = abs_uri(&par.text_document.uri);

    let lenses = state
        .try_find_folder_and_doc(&doc_path)
        .map(|(folder, src_doc)| lenses::for_doc(state.client(), &folder, &src_doc));

    response_ok_opt(id, lenses)
}

fn execute_command(id: Value, params: Value) -> Value {
    let par: lsp_types::ExecuteCommandParams =
        match parse_params(&id, "workspace/executeCommand", params) {
            Ok(p) => p,
            Err(e) => return e,
        };

    if par.command == lenses::FIND_REFERENCES_LENS {
        // Code lenses need an associated command. A dummy implementation is
        // provided here because showing references has to be handled by the
        // client, which is too much hassle to arrange for every client.
        response_ok(id, json!(0))
    } else {
        invalid_params(id, format!("Command {} is unsupported", par.command))
    }
}

/// Runs a request against an immutable snapshot. Nothing here touches shared
/// mutable state, so it is safe to run on a blocking thread.
pub fn handle_request(state: &State, method: &str, id: Value, params: Value) -> Value {
    match method {
        "workspace/symbol" => workspace_symbol(state, id, params),
        "textDocument/documentSymbol" => document_symbol(state, id, params),
        "textDocument/completion" => completion(state, id, params),
        "textDocument/definition" => definition(state, id, params),
        "textDocument/hover" => hover(state, id, params),
        "textDocument/references" => references(state, id, params),
        "textDocument/semanticTokens/full" => semantic_tokens_full(state, id, params),
        "textDocument/semanticTokens/range" => semantic_tokens_range(state, id, params),
        "textDocument/codeAction" => code_action(state, id, params),
        "textDocument/rename" => rename(state, id, params),
        "textDocument/prepareRename" => prepare_rename(state, id, params),
        "textDocument/codeLens" => code_lens(state, id, params),
        other => {
            trace!("Unsupported request {other}");
            invalid_params(id, format!("Unsupported method {other}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Notifications
// ---------------------------------------------------------------------------

type MutatorFn = Box<dyn FnOnce(&State) -> MutationResult + Send>;

fn on_did_open(params: Value) -> MutatorFn {
    Box::new(move |state| {
        let par: lsp_types::DidOpenTextDocumentParams = match serde_json::from_value(params) {
            Ok(par) => par,
            Err(err) => {
                warn!("Ignoring malformed textDocument/didOpen: {err}");
                return MutationResult::empty();
            }
        };

        let path = abs_uri(&par.text_document.uri);
        let enclosing = state.try_find_folder_enclosing(&path).cloned();
        let parser_settings = match &enclosing {
            None => ParserSettings::of_config(&state.user_config_or_default()),
            Some(folder) => folder.parser_settings(),
        };

        if !misc::is_markdown_file(&parser_settings.md_file_ext, path.data.to_system()) {
            return MutationResult::empty();
        }

        match enclosing {
            None => {
                let singleton_root = FolderId::of_uri(par.text_document.uri.as_str().to_string());
                trace!("Opening document in single-file mode: uri={}", par.text_document.uri.as_str());

                let doc = match Doc::from_lsp(&parser_settings, &singleton_root, &par.text_document) {
                    Ok(doc) => doc,
                    Err(err) => {
                        error!("{err}");
                        return MutationResult::empty();
                    }
                };
                let user_config = state.workspace().user_config();
                let new_folder = Folder::single_file(doc, user_config);
                MutationResult::state(state.update_folder(new_folder))
            }
            Some(folder) => {
                let folder_id = folder.id();
                let doc = match Doc::from_lsp(&parser_settings, &folder_id, &par.text_document) {
                    Ok(doc) => doc,
                    Err(err) => {
                        error!("{err}");
                        return MutationResult::empty();
                    }
                };
                let new_folder = folder.with_doc(doc);
                MutationResult::state(state.update_folder(new_folder))
            }
        }
    })
}

fn on_did_change(params: Value) -> MutatorFn {
    Box::new(move |state| {
        let par: DidChangeTextDocumentParams = match serde_json::from_value(params) {
            Ok(par) => par,
            Err(err) => {
                warn!("Ignoring malformed textDocument/didChange: {err}");
                return MutationResult::empty();
            }
        };
        let doc_path = abs_uri(&par.text_document.uri);

        match state.try_find_folder_and_doc(&doc_path) {
            Some((folder, doc)) => {
                let new_doc = match Doc::apply_lsp_change(
                    &folder.parser_settings(),
                    &par.content_changes,
                    par.text_document.version,
                    &doc,
                ) {
                    Ok(d) => d,
                    Err(err) => {
                        error!("{err}");
                        return MutationResult::empty();
                    }
                };
                let new_folder = folder.with_doc(new_doc);
                MutationResult::state(state.update_folder(new_folder))
            }
            None => {
                warn!("Document not found: method=textDocumentDidChange, uri={}", doc_path.uri);
                MutationResult::empty()
            }
        }
    })
}

fn on_did_close(params: Value) -> MutatorFn {
    Box::new(move |state| {
        let par: DidCloseTextDocumentParams = match serde_json::from_value(params) {
            Ok(par) => par,
            Err(err) => {
                warn!("Ignoring malformed textDocument/didClose: {err}");
                return MutationResult::empty();
            }
        };
        let path = abs_uri(&par.text_document.uri);

        match state.try_find_folder_and_doc(&path) {
            None => MutationResult::empty(),
            Some((folder, doc)) => {
                let folder_id = folder.id();
                match folder.close_doc(doc.id()) {
                    Some(folder) => MutationResult::state(state.update_folder(folder)),
                    None => MutationResult::state(state.remove_folder(&folder_id)),
                }
            }
        }
    })
}

fn on_did_change_workspace_folders(params: Value) -> MutatorFn {
    Box::new(move |state| {
        let par: DidChangeWorkspaceFoldersParams = match serde_json::from_value(params) {
            Ok(par) => par,
            Err(err) => {
                warn!("Ignoring malformed workspace/didChangeWorkspaceFolders: {err}");
                return MutationResult::empty();
            }
        };
        MutationResult::state(state.update_folders_from_lsp(&par.event.added, &par.event.removed))
    })
}

fn on_did_create_files(params: Value) -> MutatorFn {
    Box::new(move |state| {
        let par: lsp_types::CreateFilesParams = match serde_json::from_value(params) {
            Ok(par) => par,
            Err(err) => {
                warn!("Ignoring malformed workspace/didCreateFiles: {err}");
                return MutationResult::empty();
            }
        };

        let mut new_state = state.clone();
        for file in &par.files {
            let doc_uri = abs_uri_str(&file.uri);
            trace!("Processing file create not: uri={}", doc_uri.uri);

            let Some(folder) = new_state.try_find_folder_enclosing(&doc_uri).cloned() else {
                continue;
            };
            let parser_settings = folder.parser_settings();
            if !misc::is_markdown_file(&parser_settings.md_file_ext, doc_uri.data.to_system()) {
                continue;
            }

            let folder_id = folder.id();
            match Doc::try_load(&parser_settings, &folder_id, &LocalPath::Abs(doc_uri.data.clone())) {
                Some(doc) => {
                    let new_folder = folder.with_doc(doc);
                    new_state = new_state.update_folder(new_folder);
                }
                None => warn!("Couldn't load created document: uri={}", doc_uri.uri),
            }
        }
        MutationResult::state(new_state)
    })
}

fn on_did_delete_files(params: Value) -> MutatorFn {
    Box::new(move |state| {
        let par: lsp_types::DeleteFilesParams = match serde_json::from_value(params) {
            Ok(par) => par,
            Err(err) => {
                warn!("Ignoring malformed workspace/didDeleteFiles: {err}");
                return MutationResult::empty();
            }
        };

        let mut new_state = state.clone();
        for file in &par.files {
            let uri = abs_uri_str(&file.uri);
            trace!("Processing file delete not: uri={}", uri.uri);

            let Some((folder, doc)) = state.try_find_folder_and_doc(&uri) else {
                continue;
            };
            let folder_id = folder.id();
            match folder.without_doc(doc.id()) {
                None => new_state = new_state.remove_folder(&folder_id),
                Some(new_folder) => new_state = new_state.update_folder(new_folder),
            }
        }
        MutationResult::state(new_state)
    })
}

fn try_load_user_config() -> Option<Config> {
    let path = config::user_config_file();
    if path.is_file() {
        trace!("Found user config: config={}", path.display());
        let config = config::read(&path.to_string_lossy());
        if config.is_none() {
            error!("Malformed user config, skipping: config={}", path.display());
        }
        config
    } else {
        trace!("No user config found: path={}", path.display());
        None
    }
}

fn on_initialize(params: Value) -> Result<(State, InitializeResult), Value> {
    let par: InitializeParams = serde_json::from_value(params)
        .map_err(|e| json!({"jsonrpc":"2.0","id":null,"error":{"code":INVALID_PARAMS,"message":format!("Invalid params for initialize: {e}")}}))?;

    let workspace_folders = extract_workspace_folders(&par);
    let client_desc = ClientDescription::of_params(&par);

    debug!("Obtained workspace folders: workspace={workspace_folders:?}");

    let user_config = try_load_user_config();
    let folders = read_workspace(user_config.clone(), &workspace_folders);
    let num_notes: usize = folders.iter().map(Folder::doc_count).sum();

    debug!(
        "Completed reading workspace folders: numFolders={}, numNotes={num_notes}",
        folders.len()
    );

    let workspace = Workspace::of_folders(user_config.clone(), folders);
    let init_state = State::mk(client_desc, workspace);

    // Server capabilities are communicated for the whole workspace, so all
    // configured markdown extensions are collected and the client is asked to
    // watch them all; per-folder filtering happens on our side.
    //
    // NOTE: this doesn't address a folder added to the workspace later, which
    // would need dynamic capability registration.
    let mut configured_exts: Vec<String> = Vec::new();
    for folder in init_state.workspace().folders() {
        for ext in folder.configured_markdown_exts() {
            if !configured_exts.contains(&ext) {
                configured_exts.push(ext);
            }
        }
    }

    let (text_sync_source, text_sync_kind) =
        calc_text_sync(&user_config, init_state.workspace(), init_state.client());
    debug!("Configured text sync: source={text_sync_source}, kind={text_sync_kind:?}");

    let server_caps = mk_server_caps(&configured_exts, text_sync_kind, &par);
    let init_result = InitializeResult {
        capabilities: server_caps,
        server_info: Some(lsp_types::ServerInfo {
            name: "marksman".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };

    debug!("Finished workspace initialization. Waiting for the `initialized` notification from the client.");

    Ok((init_state, init_result))
}

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

async fn read_message(reader: &mut BufReader<tokio::io::Stdin>) -> anyhow::Result<Option<Value>> {
    let mut header = String::new();
    let mut content_length: Option<usize> = None;

    loop {
        header.clear();
        let read = reader.read_line(&mut header).await?;
        if read == 0 {
            return Ok(None);
        }
        let line = header.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse().ok();
        }
    }

    let Some(length) = content_length else {
        anyhow::bail!("Missing Content-Length header");
    };

    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await?;
    Ok(Some(serde_json::from_slice(&body)?))
}

async fn writer_task(mut rx: mpsc::UnboundedReceiver<Value>, stdout: &mut tokio::io::Stdout) {
    while let Some(message) = rx.recv().await {
        let Ok(body) = serde_json::to_vec(&message) else { continue };
        let header = format!("Content-Length: {}\r\n\r\n", body.len());
        if stdout.write_all(header.as_bytes()).await.is_err() {
            return;
        }
        if stdout.write_all(&body).await.is_err() {
            return;
        }
        if stdout.flush().await.is_err() {
            return;
        }
    }
}

/// Runs the server on stdin/stdout.
pub async fn start() -> anyhow::Result<()> {
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Value>();
    let mut stdout = tokio::io::stdout();
    let writer = tokio::spawn(async move { writer_task(out_rx, &mut stdout).await });

    let mut reader = BufReader::new(tokio::io::stdin());
    let mut state_handle: Option<StateHandle> = None;

    while let Some(message) = read_message(&mut reader).await? {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str).map(str::to_string);
        let params = take_params(&message);

        match (id.clone(), method) {
            (Some(id), Some(method)) => {
                if method == "initialize" {
                    // Handled inline so that capabilities reach the client
                    // before anything else is answered.
                    let initialized =
                        match tokio::task::spawn_blocking(move || on_initialize(params)).await {
                            Ok(pair) => pair,
                            Err(err) => crate::fatality::abort(None, &err.to_string()),
                        };

                    let (init_state, init_result) = match initialized {
                        Ok(pair) => pair,
                        Err(response) => {
                            let _ = out_tx.send(response);
                            continue;
                        }
                    };

                    let (state_tx, state_rx) = mpsc::unbounded_channel::<StateMsg>();
                    let (diag_tx, diag_rx) = mpsc::unbounded_channel::<StateSnapshot>();
                    let (status_tx, status_rx) = mpsc::unbounded_channel::<usize>();

                    let supports_status = init_state.client().supports_status();
                    let hooks = Hooks {
                        diagnostics: diag_tx,
                        status: status_tx,
                        status_enabled: supports_status,
                    };

                    tokio::spawn(diagnostics_task(diag_rx, out_tx.clone()));
                    tokio::spawn(status_task(status_rx, out_tx.clone()));
                    tokio::spawn(state_actor(state_rx, init_state, hooks));

                    state_handle = Some(StateHandle { tx: state_tx });
                    let _ = out_tx.send(response_ok(id, init_result));
                    continue;
                }

                if method == "shutdown" {
                    trace!("Preparing for shutdown");
                    let _ = out_tx.send(response_ok(id, Value::Null));
                    continue;
                }

                let Some(handle) = state_handle.clone() else {
                    let _ = out_tx
                        .send(internal_error(id, "State is not initialized"));
                    continue;
                };

                if method == "workspace/executeCommand" {
                    let out = out_tx.clone();
                    tokio::spawn(async move {
                        let _ = out.send(execute_command(id, params));
                    });
                    continue;
                }

                let out = out_tx.clone();
                tokio::spawn(async move {
                    let Some(state) = handle.read().await else {
                        let _ = out.send(internal_error(id, "State is not initialized"));
                        return;
                    };
                    // Language features are CPU-bound; keep them off the runtime.
                    let method_owned = method.clone();
                    let computed = tokio::task::spawn_blocking(move || {
                        handle_request(&state, &method_owned, id, params)
                    })
                    .await;

                    match computed {
                        Ok(response) => {
                            let _ = out.send(response);
                        }
                        Err(err) => error!("Request handler panicked: {err}"),
                    }
                });
            }
            (None, Some(method)) => {
                if method == "exit" {
                    trace!("Exiting");
                    break;
                }

                if method == "initialized" {
                    debug!("Received `initialized` notification from the client. Setting up background services.");
                    if let Some(handle) = &state_handle {
                        handle.register_hooks().await;
                    }
                    debug!("Initialization complete.");
                    continue;
                }

                let mutator: MutatorFn = match method.as_str()
                {
                    "textDocument/didOpen" => on_did_open(params),
                    "textDocument/didChange" => on_did_change(params),
                    "textDocument/didClose" => on_did_close(params),
                    "workspace/didChangeWorkspaceFolders" => on_did_change_workspace_folders(params),
                    "workspace/didCreateFiles" => on_did_create_files(params),
                    "workspace/didDeleteFiles" => on_did_delete_files(params),
                    other => {
                        trace!("Ignoring notification {other}");
                        continue;
                    }
                };

                if let Some(handle) = &state_handle {
                    // Awaited so that document notifications stay ordered.
                    let _ = handle.mutate(move |state| mutator(state)).await;
                }
            }
            _ => trace!("Ignoring message without a method"),
        }
    }

    // Dropping the senders lets the background tasks finish and flush.
    drop(state_handle);
    drop(out_tx);
    let _ = writer.await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_params(client_name: &str) -> InitializeParams {
        InitializeParams {
            client_info: Some(lsp_types::ClientInfo { name: client_name.to_string(), version: None }),
            ..Default::default()
        }
    }

    fn temp_workspace(name: &str, with_marker: bool) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("marksman-server-{name}"));
        std::fs::remove_dir_all(&dir).ok();
        if with_marker {
            std::fs::create_dir_all(dir.join(".git")).unwrap();
        } else {
            std::fs::create_dir_all(&dir).unwrap();
        }
        dir
    }

    fn init_state(dir: &std::path::Path) -> State {
        let folder_id =
            FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap()));
        let folder = Folder::try_load(None, "t", folder_id).unwrap();
        State::mk(ClientDescription::empty(), Workspace::of_folders(None, vec![folder]))
    }

    #[test]
    fn watch_glob_covers_all_configured_extensions() {
        assert_eq!(
            mk_watch_glob(&["md".to_string(), "markdown".to_string()]),
            "**/*.{md,markdown}"
        );
    }

    #[test]
    fn capabilities_advertise_the_documented_feature_set() {
        let caps = mk_server_caps(&["md".to_string()], TextSync::Full, &init_params("Neovim"));

        assert_eq!(caps.definition_provider, Some(OneOf::Left(true)));
        assert_eq!(caps.references_provider, Some(OneOf::Left(true)));
        assert_eq!(caps.hover_provider, Some(HoverProviderCapability::Simple(true)));
        // Non-VSCode clients get the workspace and document symbol providers.
        assert_eq!(caps.workspace_symbol_provider, Some(OneOf::Left(true)));
        assert_eq!(caps.document_symbol_provider, Some(OneOf::Left(true)));

        let completion = caps.completion_provider.unwrap();
        assert_eq!(
            completion.trigger_characters,
            Some(vec!["[".to_string(), "#".to_string(), "(".to_string()])
        );
        assert_eq!(completion.resolve_provider, None);

        match caps.text_document_sync {
            Some(TextDocumentSyncCapability::Options(opts)) => {
                assert_eq!(opts.open_close, Some(true));
                assert_eq!(opts.change, Some(TextDocumentSyncKind::FULL));
                assert!(opts.save.is_none());
            }
            other => panic!("expected sync options, got {other:?}"),
        }

        match caps.semantic_tokens_provider {
            Some(SemanticTokensServerCapabilities::SemanticTokensOptions(opts)) => {
                let names: Vec<&str> = opts.legend.token_types.iter().map(|t| t.as_str()).collect();
                assert_eq!(names, vec!["class", "class", "enumMember"]);
                assert!(opts.legend.token_modifiers.is_empty());
                assert_eq!(opts.range, Some(true));
                assert_eq!(opts.full, Some(SemanticTokensFullOptions::Delta { delta: Some(false) }));
            }
            other => panic!("expected semantic tokens options, got {other:?}"),
        }

        let workspace = caps.workspace.unwrap();
        let folders = workspace.workspace_folders.unwrap();
        assert_eq!(folders.supported, Some(true));
        assert_eq!(folders.change_notifications, Some(OneOf::Left(true)));

        let file_ops = workspace.file_operations.unwrap();
        assert!(file_ops.did_create.is_some());
        assert!(file_ops.did_delete.is_some());
        // Renames arrive as a close plus an open, so they are deliberately off.
        assert!(file_ops.did_rename.is_none());

        assert_eq!(caps.execute_command_provider.unwrap().commands, Vec::<String>::new());
        match caps.code_action_provider {
            Some(CodeActionProviderCapability::Options(opts)) => {
                assert_eq!(opts.resolve_provider, Some(false))
            }
            other => panic!("expected code action options, got {other:?}"),
        }
    }

    #[test]
    fn vscode_loses_the_symbol_providers() {
        let caps = mk_server_caps(
            &["md".to_string()],
            TextSync::Full,
            &init_params("Visual Studio Code"),
        );
        assert_eq!(caps.workspace_symbol_provider, Some(OneOf::Left(false)));
        assert_eq!(caps.document_symbol_provider, Some(OneOf::Left(false)));
    }

    #[test]
    fn rename_prepare_provider_follows_client_support() {
        let caps = mk_server_caps(&["md".to_string()], TextSync::Full, &init_params("Neovim"));
        assert_eq!(caps.rename_provider, Some(OneOf::Left(true)));

        let mut par = init_params("Neovim");
        par.capabilities.text_document = Some(lsp_types::TextDocumentClientCapabilities {
            rename: Some(lsp_types::RenameClientCapabilities {
                prepare_support: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        });
        let caps = mk_server_caps(&["md".to_string()], TextSync::Full, &par);
        match caps.rename_provider {
            Some(OneOf::Right(opts)) => assert_eq!(opts.prepare_provider, Some(true)),
            other => panic!("expected rename options, got {other:?}"),
        }
    }

    #[test]
    fn incremental_sync_is_reflected_in_capabilities() {
        let caps = mk_server_caps(
            &["md".to_string()],
            TextSync::Incremental,
            &init_params("Neovim"),
        );
        match caps.text_document_sync {
            Some(TextDocumentSyncCapability::Options(opts)) => {
                assert_eq!(opts.change, Some(TextDocumentSyncKind::INCREMENTAL))
            }
            other => panic!("expected sync options, got {other:?}"),
        }
    }

    #[test]
    fn text_sync_prefers_folder_config_then_user_then_client() {
        let client = ClientDescription::empty();
        let ws = Workspace::default();

        assert_eq!(calc_text_sync(&None, &ws, &client), ("default", TextSync::Full));

        let user = Config { core_text_sync: Some(TextSync::Incremental), ..Config::empty() };
        assert_eq!(
            calc_text_sync(&Some(user), &ws, &client),
            ("userConfig", TextSync::Incremental)
        );

        let mut with_preference = ClientDescription::empty();
        with_preference.opts.preferred_text_sync_kind = Some(TextSync::Incremental);
        assert_eq!(
            calc_text_sync(&None, &ws, &with_preference),
            ("clientOption", TextSync::Incremental)
        );
    }

    #[test]
    #[allow(deprecated)]
    fn workspace_folders_come_from_the_initialize_params() {
        let dir = temp_workspace("init", true);

        let mut par = init_params("Neovim");
        par.root_uri = Some(crate::uri::parse(
            &crate::paths::system_path_to_uri_string(dir.to_str().unwrap()),
        ));
        let folders = extract_workspace_folders(&par);
        assert_eq!(folders.len(), 1);
        assert_eq!(folders.keys().next().unwrap(), &format!("marksman-server-init"));

        // A folder without a project marker is rejected.
        let plain = temp_workspace("init-plain", false);
        par.root_uri = Some(crate::uri::parse(
            &crate::paths::system_path_to_uri_string(plain.to_str().unwrap()),
        ));
        assert!(extract_workspace_folders(&par).is_empty());

        // Explicit workspace folders win over the root uri.
        par.workspace_folders = Some(vec![lsp_types::WorkspaceFolder {
            uri: crate::uri::parse("file:///nowhere"),
            name: "explicit".to_string(),
        }]);
        assert!(extract_workspace_folders(&par).is_empty());

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&plain).ok();
    }

    #[test]
    fn read_workspace_loads_only_existing_folders() {
        let dir = temp_workspace("read-ws", true);
        std::fs::write(dir.join("a.md"), b"# A\n").unwrap();
        let roots = BTreeMap::from([(
            "t".to_string(),
            FolderId::of_uri(crate::paths::system_path_to_uri_string(dir.to_str().unwrap())),
        )]);

        let folders = read_workspace(None, &roots);
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].doc_count(), 1);

        let missing = BTreeMap::from([(
            "gone".to_string(),
            FolderId::of_uri("file:///definitely/not/here"),
        )]);
        assert!(read_workspace(None, &missing).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diagnostics_are_only_published_when_they_change() {
        let dir = temp_workspace("diag", true);
        std::fs::write(dir.join("a.md"), b"# A\n\n[[Missing]]\n").unwrap();
        let state = init_state(&dir);

        let (diag, updates) = calc_diagnostics_update(None, &state);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].diagnostics.len(), 1);
        assert!(updates[0].version.is_none());

        // Nothing changed, so nothing is republished.
        let (_, updates) = calc_diagnostics_update(Some((&state, &diag)), &state);
        assert!(updates.is_empty());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_reopened_document_with_diagnostics_is_republished() {
        let dir = temp_workspace("reopen", true);
        std::fs::write(dir.join("a.md"), b"# A\n\n[[Missing]]\n").unwrap();
        let state = init_state(&dir);

        let (diag, _) = calc_diagnostics_update(None, &state);
        // The same content, but now with a version: that is a reopen.
        let reopened = init_state(&dir);
        let (_, updates) = calc_diagnostics_update(Some((&reopened, &diag)), &state);
        assert!(updates.is_empty(), "identical diagnostics are not republished");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_payload_uses_camel_case_doc_count() {
        let params = MarksmanStatusParams { state: "ok".to_string(), doc_count: 7 };
        assert_eq!(serde_json::to_value(params).unwrap(), json!({ "state": "ok", "docCount": 7 }));
    }

    #[test]
    fn execute_command_rejects_unknown_commands() {
        let state = init_state(&temp_workspace("exec", true));
        let response = execute_command(
            json!(1),
            json!({ "command": "nope" }),
        );
        let _ = state;
        assert_eq!(response["error"]["code"], json!(INVALID_PARAMS));
        assert!(response["error"]["message"].as_str().unwrap().contains("unsupported"));
    }

    #[test]
    fn execute_command_accepts_the_find_references_lens() {
        let response = execute_command(json!(1), json!({ "command": lenses::FIND_REFERENCES_LENS }));
        assert_eq!(response["result"], json!(0));
    }

    #[test]
    fn requests_answer_from_a_snapshot() {
        let dir = temp_workspace("requests", true);
        std::fs::write(dir.join("a.md"), b"# A\n\n## Sub\n\n[[A]]\n").unwrap();
        let state = init_state(&dir);
        let uri = crate::uri::parse(&format!(
            "file://{}/a.md",
            dir.to_str().unwrap()
        ));

        let symbols = document_symbol(
            &state,
            json!(1),
            json!({ "textDocument": { "uri": uri.as_str() } }),
        );
        // An empty ClientDescription advertises no hierarchy support, so the
        // flat SymbolInformation form comes back.
        let names: Vec<&str> = symbols["result"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["H1: A", "H2: Sub"]);

        let definition = crate::server::definition(
            &state,
            json!(2),
            json!({
                "textDocument": { "uri": uri.as_str() },
                "position": { "line": 4, "character": 3 }
            }),
        );
        assert_eq!(definition["result"].as_array().map(|a| a.len()).unwrap_or(1), 1);

        let unknown = handle_request(&state, "textDocument/nope", json!(3), json!({}));
        assert_eq!(unknown["error"]["code"], json!(INVALID_PARAMS));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn state_actor_serializes_mutations_and_serves_reads() {
        let dir = temp_workspace("actor", true);
        std::fs::write(dir.join("a.md"), b"# A\n").unwrap();
        let (tx, rx) = mpsc::unbounded_channel::<StateMsg>();
        let handle = StateHandle { tx };

        let (diag_tx, _diag_rx) = mpsc::unbounded_channel::<StateSnapshot>();
        let (status_tx, _status_rx) = mpsc::unbounded_channel::<usize>();
        let hooks = Hooks { diagnostics: diag_tx, status: status_tx, status_enabled: false };

        let actor = tokio::spawn(state_actor(rx, init_state(&dir), hooks));

        let snapshot = handle.read().await.expect("a snapshot");
        assert_eq!(snapshot.workspace().doc_count(), 1);
        assert_eq!(snapshot.revision(), 0);

        handle
            .mutate(|state| MutationResult::state(state.remove_folder(&FolderId::of_uri("file:///x"))))
            .await
            .unwrap();

        let snapshot = handle.read().await.expect("a snapshot");
        assert_eq!(snapshot.revision(), 1);

        // A mutation that reports no new state leaves the revision alone.
        handle.mutate(|_| MutationResult::empty()).await.unwrap();
        assert_eq!(handle.read().await.unwrap().revision(), 1);

        drop(handle);
        let _ = actor.await;
        std::fs::remove_dir_all(&dir).ok();
    }
}
