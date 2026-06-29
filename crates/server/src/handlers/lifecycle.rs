//! Lifecycle handlers: initialize, initialized, shutdown.

use crate::handlers::helpers;
use crate::lsp_ext::{
    KnotClientReadyParams, KnotClientReadyResponse, KnotFormatSwitchCompleteParams,
    KnotFormatSwitchCompleteResponse,
};
use crate::state::ServerState;
use knot_formats::plugin as fmt_plugin;
use lsp_types::*;
use std::time::Duration;

pub(crate) async fn initialize(
    state: &ServerState,
    params: InitializeParams,
) -> Result<InitializeResult, tower_lsp::jsonrpc::Error> {
    tracing::info!("initialize");

    // Reset the shutdown guard — we're starting fresh
    state
        .shutting_down
        .store(false, std::sync::atomic::Ordering::SeqCst);

    // Update workspace root URI if provided
    if let Some(root_uri) = params.root_uri {
        let mut inner = state.inner.write().await;
        inner.workspace = knot_core::Workspace::new(root_uri);
    }

    // Read the global storage path from initialization options (sent by
    // the VS Code extension). This is the root for the extension-managed
    // toolchain: `<global_storage>/tweego/` for the binary,
    // `<global_storage>/storyformats/<id>@<ver>/` for versioned format cache.
    //
    // Also read the VS Code `knot.indexing.maxFiles` setting. Indexing
    // exclusions have been removed — the server now indexes all .tw/.twee/.js
    // files in the workspace, capped by maxFiles. Users who need to keep
    // certain directories out of the build should set `knot.build.outputDir`
    // outside the workspace root.
    if let Some(opts) = params.initialization_options {
        if let Some(path_str) = opts.get("globalStoragePath").and_then(|v| v.as_str()) {
            let path = std::path::PathBuf::from(path_str);
            let mut inner = state.inner.write().await;
            inner.global_storage_path = Some(path);
            tracing::info!(
                "Extension global storage path: {:?}",
                inner.global_storage_path
            );
        }

        // Read the VS Code maxFiles setting. This is only set if not
        // already configured in knot.json (knot.json takes priority for
        // project-specific limits).
        let vc_max_files: Option<usize> = opts
            .get("indexingMaxFiles")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize);

        if let Some(max) = vc_max_files {
            let mut inner = state.inner.write().await;
            if inner.workspace.config.max_files.is_none() {
                inner.workspace.config.max_files = Some(max);
            }
            tracing::info!("Indexing max_files from VS Code: {:?}", vc_max_files);
        }
    }

    // Load workspace configuration from .vscode/knot.json
    {
        let inner = state.inner.read().await;
        let root_uri = &inner.workspace.root_uri;
        if let Ok(root_path) = root_uri.to_file_path() {
            let config_path = root_path.join(".vscode").join("knot.json");
            if config_path.exists() {
                drop(inner);
                let mut inner = state.inner.write().await;
                if let Ok(config_text) = std::fs::read_to_string(&config_path) {
                    if let Err(e) = inner.workspace.load_config(&config_text) {
                        tracing::warn!("Failed to load knot.json: {}", e);
                    } else {
                        tracing::info!("Loaded .vscode/knot.json configuration");
                    }
                }
            }
        }
    }

    let capabilities = ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                will_save: Some(false),
                will_save_wait_until: Some(false),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
            },
        )),
        completion_provider: Some(CompletionOptions {
            resolve_provider: Some(true),
            trigger_characters: Some(vec![
                "[".to_string(),
                "$".to_string(),
                "_".to_string(),
                "<".to_string(),
                ".".to_string(),
                "\"".to_string(),
                "?".to_string(),
            ]),
            work_done_progress_options: Default::default(),
            all_commit_characters: None,
            completion_item: None,
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        declaration_provider: Some(DeclarationCapability::Simple(true)),
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        // Document highlight: highlights all occurrences of the symbol under
        // the cursor in the current file. Fires on cursor move. Supported
        // symbol types: passages (header + links), custom macros, functions,
        // templates, and variables (with Read/Write kind distinction).
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec![" ".to_string(), ",".to_string()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::QUICKFIX, CodeActionKind::REFACTOR]),
            work_done_progress_options: Default::default(),
            resolve_provider: Some(false),
        })),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        inlay_hint_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: None,
            work_done_progress_options: Default::default(),
        }),
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
            first_trigger_character: "]".to_string(),
            more_trigger_character: Some(vec![">".to_string()]),
        }),
        linked_editing_range_provider: Some(LinkedEditingRangeServerCapabilities::Simple(true)),
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        diagnostic_provider: None,
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                work_done_progress_options: Default::default(),
                legend: SemanticTokensLegend {
                    token_types: fmt_plugin::SemanticTokenType::all_types()
                        .iter()
                        .map(|t| lsp_types::SemanticTokenType::new(t.lsp_name()))
                        .collect(),
                    token_modifiers: fmt_plugin::SemanticTokenModifier::all_modifiers()
                        .iter()
                        .map(|m| lsp_types::SemanticTokenModifier::new(m.lsp_name()))
                        .collect(),
                },
                range: Some(false),
                full: Some(SemanticTokensFullOptions::Bool(true)),
            },
        )),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: None,
            file_operations: None,
        }),
        ..Default::default()
    };

    Ok(InitializeResult {
        capabilities,
        server_info: Some(ServerInfo {
            name: "Knot Language Server".to_string(),
            version: Some("2.0.0".to_string()),
        }),
    })
}

pub(crate) async fn initialized(state: &ServerState, _params: InitializedParams) {
    tracing::info!("Language server initialized");

    state
        .client
        .log_message(
            MessageType::INFO,
            "Knot Language Server initialized — waiting for clientReady…",
        )
        .await;

    // Register for configuration change notifications
    state
        .client
        .register_capability(vec![Registration {
            id: "knot-didChangeConfiguration".to_string(),
            method: "workspace/didChangeConfiguration".to_string(),
            register_options: None,
        }])
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to register didChangeConfiguration: {}", e);
        });

    // Register file watchers for .tw/.twee files
    register_file_watchers(&state.client).await;

    // Spawn workspace indexing in a background task that WAITS for the
    // extension to confirm it's ready via `knot/clientReady`. This
    // eliminates the race where the server sends `formatDetected` before
    // the extension has registered notification handlers.
    let inner = state.inner.clone();
    let client = state.client.clone();
    let client_ready = state.client_ready.clone();

    tokio::spawn(async move {
        tracing::info!("Indexing task spawned — waiting for clientReady handshake");

        // Wait for the extension to signal readiness, with a 30-second
        // safety timeout in case the extension never sends clientReady
        // (e.g., older extension version).
        tokio::select! {
            _ = client_ready.notified() => {
                tracing::info!("clientReady received — starting workspace indexing");
            }
            _ = tokio::time::sleep(Duration::from_secs(30)) => {
                tracing::warn!("clientReady not received after 30s, starting indexing anyway");
            }
        }

        if let Err(e) = helpers::index_workspace(&inner, &client).await {
            tracing::error!("Workspace indexing failed: {}", e);
            client
                .log_message(
                    MessageType::ERROR,
                    format!("Workspace indexing failed: {}", e),
                )
                .await;
        } else {
            client
                .log_message(MessageType::INFO, "Workspace indexing complete")
                .await;
        }
    });
}

pub(crate) async fn shutdown(state: &ServerState) -> Result<(), tower_lsp::jsonrpc::Error> {
    tracing::info!("Language server shutting down");
    // Signal in-flight handlers to short-circuit so they don't try to
    // write to a transport stream that is about to be destroyed.
    state
        .shutting_down
        .store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Register file watchers for .tw, .twee, and .js files using the
/// `client/registerCapability` LSP request.
///
/// `.js` files are watched because Tweego bundles them from the source
/// directory as `<script>` tags — Knot indexes and analyzes them the
/// same way as `[script]`-tagged passages.
async fn register_file_watchers(client: &tower_lsp::Client) {
    let watchers = vec![
        FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*.tw".to_string()),
            kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
        },
        FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*.twee".to_string()),
            kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
        },
        FileSystemWatcher {
            glob_pattern: GlobPattern::String("**/*.js".to_string()),
            kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
        },
    ];

    let registrations = vec![Registration {
        id: "knot-watch-twee-files".to_string(),
        method: "workspace/didChangeWatchedFiles".to_string(),
        register_options: Some(
            serde_json::to_value(DidChangeWatchedFilesRegistrationOptions { watchers })
                .unwrap_or_else(|e| {
                    tracing::warn!(
                        "Failed to serialize DidChangeWatchedFilesRegistrationOptions: {e}"
                    );
                    serde_json::Value::Null
                }),
        ),
    }];

    if let Err(e) = client.register_capability(registrations).await {
        tracing::warn!("Failed to register file watchers: {}", e);
    } else {
        tracing::info!("Registered file watchers for .tw/.twee/.js files");
    }
}

// ---------------------------------------------------------------------------
// Custom LSP method handlers: knot/clientReady, knot/formatSwitchComplete
// ---------------------------------------------------------------------------

impl ServerState {
    /// Handler for `knot/clientReady` custom request.
    ///
    /// The extension sends this after all notification handlers are
    /// registered. The server's `initialized` handler spawns an indexing
    /// task that waits on `client_ready.notified()` before starting,
    /// preventing the race where `formatDetected` arrives before the
    /// extension has registered notification handlers.
    pub async fn knot_client_ready(
        &self,
        _params: KnotClientReadyParams,
    ) -> Result<KnotClientReadyResponse, tower_lsp::jsonrpc::Error> {
        tracing::info!("knot/clientReady received — notifying indexing task");
        self.client_ready.notify_one();
        Ok(KnotClientReadyResponse { acknowledged: true })
    }

    /// Handler for `knot/formatSwitchComplete` custom request.
    ///
    /// The extension sends this after all document language IDs have been
    /// switched following a `formatDetected` notification. The server
    /// sends ONE unified `workspace/semanticTokens/refresh` to ensure all
    /// visible editors get fresh tokens after the language ID cascade.
    ///
    /// Note: Format is frozen after initial indexing — no dynamic format
    /// switches are possible. This handshake only fires once, during the
    /// initial `formatDetected` cascade after workspace indexing.
    pub async fn knot_format_switch_complete(
        &self,
        params: KnotFormatSwitchCompleteParams,
    ) -> Result<KnotFormatSwitchCompleteResponse, tower_lsp::jsonrpc::Error> {
        tracing::info!(
            "knot/formatSwitchComplete received — workspace_uri={}, switched_count={}",
            params.workspace_uri,
            params.switched_count
        );
        // Send ONE unified refresh now that the cascade is complete
        self.schedule_semantic_token_refresh().await;
        Ok(KnotFormatSwitchCompleteResponse { acknowledged: true })
    }
}
