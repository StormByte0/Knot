//! Lifecycle handlers: initialize, initialized, shutdown.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;

pub(crate) async fn initialize(
    state: &ServerState,
    params: InitializeParams,
) -> Result<InitializeResult, tower_lsp::jsonrpc::Error> {
    tracing::info!("initialize");

    // Reset the shutdown guard — we're starting fresh
    state.shutting_down.store(false, std::sync::atomic::Ordering::SeqCst);

    // Update workspace root URI if provided
    if let Some(root_uri) = params.root_uri {
        let mut inner = state.inner.write().await;
        inner.workspace = knot_core::Workspace::new(root_uri);
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
                "<".to_string(),
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
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec![" ".to_string(), ",".to_string()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        code_action_provider: Some(CodeActionProviderCapability::Options(
            CodeActionOptions {
                code_action_kinds: Some(vec![
                    CodeActionKind::QUICKFIX,
                    CodeActionKind::REFACTOR,
                ]),
                work_done_progress_options: Default::default(),
                resolve_provider: Some(false),
            },
        )),
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
        // NOTE: diagnostic_provider (pull model) is intentionally NOT registered here.
        // The server uses the push model (publish_diagnostics) exclusively.
        // Using both models simultaneously causes VS Code to display every
        // diagnostic twice — once from the push and once from the pull — which
        // makes errors appear duplicated in hover and the Problems panel.
        diagnostic_provider: None,
        semantic_tokens_provider: Some(
            SemanticTokensServerCapabilities::SemanticTokensOptions(
                SemanticTokensOptions {
                    work_done_progress_options: Default::default(),
                    legend: SemanticTokensLegend {
                        token_types: vec![
                            // ── Passage structure ──────────────────────────
                            SemanticTokenType::new("passageHeader"),         // 0  — :: prefix on regular passages
                            SemanticTokenType::new("passageName"),           // 1  — passage name on regular passages
                            SemanticTokenType::new("link"),                  // 2  — passage name in [[links]]
                            SemanticTokenType::new("passageRef"),            // 3  — implicit passage refs
                            SemanticTokenType::new("specialPassageHeader"),  // 4  — :: prefix on special passages
                            SemanticTokenType::new("specialPassage"),        // 5  — passage name on special passages
                            SemanticTokenType::new("tag"),                   // 6  — passage tags
                            // ── Code constructs ───────────────────────────
                            SemanticTokenType::new("macro"),                 // 7  — macro name
                            SemanticTokenType::new("function"),              // 8  — widget/function definition
                            SemanticTokenType::new("variable"),              // 9  — $variable
                            SemanticTokenType::new("keyword"),               // 10 — format keywords
                            SemanticTokenType::new("boolean"),               // 11 — true/false
                            SemanticTokenType::new("number"),                // 12 — numeric literals
                            SemanticTokenType::new("string"),                // 13 — string literals
                            SemanticTokenType::new("comment"),               // 14 — comments
                            SemanticTokenType::new("operator"),              // 15 — format-specific operators
                            // ── Object model ──────────────────────────────
                            SemanticTokenType::new("namespace"),             // 16 — global objects (State, Engine)
                            SemanticTokenType::new("property"),              // 17 — object properties
                        ],
                        token_modifiers: vec![
                            lsp_types::SemanticTokenModifier::DEFINITION,    // bit 0
                            lsp_types::SemanticTokenModifier::READONLY,      // bit 1
                            lsp_types::SemanticTokenModifier::DEPRECATED,    // bit 2
                            lsp_types::SemanticTokenModifier::new("controlFlow"), // bit 3
                            lsp_types::SemanticTokenModifier::STATIC,        // bit 4 — TwineCore layer scope
                            lsp_types::SemanticTokenModifier::ASYNC,         // bit 5 — StoryFormat layer scope
                            lsp_types::SemanticTokenModifier::MODIFICATION,  // bit 6 — UserDefined layer scope
                        ],
                    },
                    range: Some(false),
                    full: Some(SemanticTokensFullOptions::Bool(true)),
                },
            ),
        ),
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
            version: Some("0.3.0".to_string()),
        }),
    })
}

pub(crate) async fn initialized(
    state: &ServerState,
    _params: InitializedParams,
) {
    tracing::info!("Language server initialized");

    state.client
        .log_message(MessageType::INFO, "Knot Language Server initialized — indexing workspace…")
        .await;

    // Register for configuration change notifications
    state.client
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

    // Spawn workspace indexing in the background
    if let Err(e) = helpers::index_workspace(&state.inner, &state.client).await {
        tracing::error!("Workspace indexing failed: {}", e);
        state.client
            .log_message(MessageType::ERROR, format!("Workspace indexing failed: {}", e))
            .await;
    } else {
        state.client
            .log_message(MessageType::INFO, "Workspace indexing complete")
            .await;
    }
}

pub(crate) async fn shutdown(
    state: &ServerState,
) -> Result<(), tower_lsp::jsonrpc::Error> {
    tracing::info!("Language server shutting down");
    // Signal in-flight handlers to short-circuit so they don't try to
    // write to a transport stream that is about to be destroyed.
    state.shutting_down.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// Register file watchers for .tw and .twee files using the
/// `client/registerCapability` LSP request.
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
    ];

    let registrations = vec![Registration {
        id: "knot-watch-twee-files".to_string(),
        method: "workspace/didChangeWatchedFiles".to_string(),
        register_options: Some(serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
            watchers,
        }).unwrap_or_else(|e| {
            tracing::warn!("Failed to serialize DidChangeWatchedFilesRegistrationOptions: {e}");
            serde_json::Value::Null
        })),
    }];

    if let Err(e) = client.register_capability(registrations).await {
        tracing::warn!("Failed to register file watchers: {}", e);
    } else {
        tracing::info!("Registered file watchers for .tw/.twee files");
    }
}
