//! Lifecycle handlers: initialize, initialized, shutdown.

use crate::handlers::helpers;
use crate::state::ServerState;
use lsp_types::*;
use tower_lsp::LanguageServer;

pub(crate) async fn initialize(
    state: &ServerState,
    params: InitializeParams,
) -> Result<InitializeResult, tower_lsp::jsonrpc::Error> {
    tracing::info!("initialize");

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
                change: Some(TextDocumentSyncKind::FULL),
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
        diagnostic_provider: Some(DiagnosticServerCapabilities::Options(
            DiagnosticOptions {
                identifier: Some("knot".to_string()),
                inter_file_dependencies: true,
                workspace_diagnostics: false,
                work_done_progress_options: Default::default(),
            },
        )),
        semantic_tokens_provider: Some(
            SemanticTokensServerCapabilities::SemanticTokensOptions(
                SemanticTokensOptions {
                    work_done_progress_options: Default::default(),
                    legend: SemanticTokensLegend {
                        token_types: vec![
                            lsp_types::SemanticTokenType::NAMESPACE,   // 0 passage header
                            lsp_types::SemanticTokenType::STRING,      // 1 link
                            lsp_types::SemanticTokenType::FUNCTION,    // 2 macro
                            lsp_types::SemanticTokenType::VARIABLE,    // 3 variable
                            lsp_types::SemanticTokenType::STRING,      // 4 string
                            lsp_types::SemanticTokenType::NUMBER,      // 5 number
                            lsp_types::SemanticTokenType::COMMENT,     // 6 comment
                            lsp_types::SemanticTokenType::DECORATOR,   // 7 tag
                            lsp_types::SemanticTokenType::KEYWORD,     // 8 keyword
                            lsp_types::SemanticTokenType::KEYWORD,     // 9 boolean
                        ],
                        token_modifiers: vec![
                            lsp_types::SemanticTokenModifier::DEFINITION,  // 0
                            lsp_types::SemanticTokenModifier::READONLY,    // 1
                            lsp_types::SemanticTokenModifier::DEPRECATED,  // 2
                            lsp_types::SemanticTokenModifier::MODIFICATION,// 3 (used for ControlFlow)
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
    _state: &ServerState,
) -> Result<(), tower_lsp::jsonrpc::Error> {
    tracing::info!("Language server shutting down");
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
        }).unwrap()),
    }];

    if let Err(e) = client.register_capability(registrations).await {
        tracing::warn!("Failed to register file watchers: {}", e);
    } else {
        tracing::info!("Registered file watchers for .tw/.twee files");
    }
}
