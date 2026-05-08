//! LSP `LanguageServer` trait implementation and helper functions.
//!
//! This module contains the core request/notification handlers that wire
//! knot-core and knot-formats into the Language Server Protocol via
//! tower-lsp. All parsing is delegated to format plugins from `knot-formats`.

use crate::lsp_ext::*;
use crate::state::ServerState;
use knot_core::editing::graph_surgery;
use knot_core::graph::{DiagnosticKind, PassageEdge, PassageNode};
use knot_core::passage::{Passage, StoryFormat};
use knot_core::workspace::StoryMetadata;
use knot_core::{AnalysisEngine, Document, Workspace};
use knot_formats::plugin as fmt_plugin;
use lsp_types::*;
use std::collections::HashMap;
use tower_lsp::LanguageServer;
use url::Url;

// ---------------------------------------------------------------------------
// Semantic-token legend indices
// ---------------------------------------------------------------------------

/// Token-type indices — must match the order in the legend we advertise.
const ST_PASSAGE_HEADER: u32 = 0;
const ST_LINK: u32 = 1;
const ST_MACRO: u32 = 2;
const ST_VARIABLE: u32 = 3;
const ST_STRING: u32 = 4;
const ST_NUMBER: u32 = 5;
const ST_COMMENT: u32 = 6;
const ST_TAG: u32 = 7;
const ST_KEYWORD: u32 = 8;
const ST_BOOLEAN: u32 = 9;

/// Token-modifier indices.
const SM_DEFINITION: u32 = 1 << 0;
const SM_READONLY: u32 = 1 << 1;
const SM_DEPRECATED: u32 = 1 << 2;
const SM_CONTROLFLOW: u32 = 1 << 3;

// ---------------------------------------------------------------------------
// LanguageServer trait implementation
// ---------------------------------------------------------------------------

#[tower_lsp::async_trait]
impl LanguageServer for ServerState {
    // -----------------------------------------------------------------------
    // Lifecycle
    // -----------------------------------------------------------------------

    async fn initialize(
        &self,
        params: InitializeParams,
    ) -> Result<InitializeResult, tower_lsp::jsonrpc::Error> {
        tracing::info!("initialize");

        // Update workspace root URI if provided
        if let Some(root_uri) = params.root_uri {
            let mut inner = self.inner.write().await;
            inner.workspace = Workspace::new(root_uri);
        }

        // Load workspace configuration from .vscode/knot.json
        {
            let inner = self.inner.read().await;
            let root_uri = &inner.workspace.root_uri;
            if let Ok(root_path) = root_uri.to_file_path() {
                let config_path = root_path.join(".vscode").join("knot.json");
                if config_path.exists() {
                    drop(inner);
                    let mut inner = self.inner.write().await;
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

    async fn initialized(&self, _params: InitializedParams) {
        tracing::info!("Language server initialized");

        self.client
            .log_message(MessageType::INFO, "Knot Language Server initialized — indexing workspace…")
            .await;

        // Register for configuration change notifications
        self.client
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
        self.register_file_watchers().await;

        // Spawn workspace indexing in the background
        let inner = &self.inner;
        let client = &self.client;

        if let Err(e) = index_workspace(inner, client).await {
            tracing::error!("Workspace indexing failed: {}", e);
            client
                .log_message(MessageType::ERROR, format!("Workspace indexing failed: {}", e))
                .await;
        } else {
            client
                .log_message(MessageType::INFO, "Workspace indexing complete")
                .await;
        }
    }

    async fn shutdown(&self) -> Result<(), tower_lsp::jsonrpc::Error> {
        tracing::info!("Language server shutting down");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Document synchronization
    // -----------------------------------------------------------------------

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        let version = params.text_document.version;

        tracing::info!("did_open: {}", uri);

        let mut inner = self.inner.write().await;
        inner.open_documents.insert(uri.clone(), text.clone());

        let format = inner.workspace.resolve_format();
        let (doc, parse_result) =
            parse_with_format_plugin(&inner.format_registry, &uri, &text, format, version);

        // Store format diagnostics for this document
        inner.format_diagnostics.insert(
            uri.clone(),
            parse_result.diagnostics.clone(),
        );

        // Check for StoryData in the newly opened document
        extract_and_set_metadata(&mut inner.workspace, &doc, &text);

        inner.workspace.insert_document(doc);
        rebuild_graph(&mut inner.workspace);

        let diagnostics = AnalysisEngine::analyze(&inner.workspace);
        let open_docs = inner.open_documents.clone();
        let fmt_diags = inner.format_diagnostics.clone();
        let config = inner.workspace.config.clone();
        drop(inner);

        publish_all_diagnostics(&self.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        let version = params.text_document.version;

        tracing::debug!("did_change: {} (v{})", uri, version);

        // With FULL sync the last change contains the full text.
        let text = params
            .content_changes
            .into_iter()
            .last()
            .map(|c| c.text)
            .unwrap_or_default();

        let mut inner = self.inner.write().await;

        // Debounce — record the edit time
        inner.debounce.record_edit();

        // Update cache
        inner.open_documents.insert(uri.clone(), text.clone());

        // Parse with format plugin
        let format = inner.workspace.resolve_format();
        let (doc, parse_result) =
            parse_with_format_plugin(&inner.format_registry, &uri, &text, format, version);

        // Update format diagnostics
        inner.format_diagnostics.insert(
            uri.clone(),
            parse_result.diagnostics.clone(),
        );

        let old_passages: Vec<Passage> = inner
            .workspace
            .get_document(&uri)
            .map(|d| d.passages.clone())
            .unwrap_or_default();
        let new_passages = doc.passages.clone();

        // Check for StoryData changes
        extract_and_set_metadata(&mut inner.workspace, &doc, &text);

        inner.workspace.insert_document(doc);
        let file_uri_str = uri.to_string();
        graph_surgery(
            &mut inner.workspace.graph,
            &old_passages,
            &new_passages,
            &file_uri_str,
        );

        // Update broken-link flags on all edges after surgery
        inner.workspace.graph.recheck_broken_links();

        let diagnostics = AnalysisEngine::analyze(&inner.workspace);
        let open_docs = inner.open_documents.clone();
        let fmt_diags = inner.format_diagnostics.clone();
        let config = inner.workspace.config.clone();
        drop(inner);

        publish_all_diagnostics(&self.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        tracing::info!("did_close: {}", uri);

        let mut inner = self.inner.write().await;
        inner.open_documents.remove(&uri);
        inner.format_diagnostics.remove(&uri);
        drop(inner);

        // Clear diagnostics for the closed file.
        self.client
            .publish_diagnostics(uri, Vec::new(), None)
            .await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        tracing::info!("did_save: {}", params.text_document.uri);
    }

    async fn did_change_configuration(&self, _params: DidChangeConfigurationParams) {
        tracing::info!("did_change_configuration");

        // Re-read .vscode/knot.json in case it was changed externally
        {
            let inner = self.inner.read().await;
            let root_uri = &inner.workspace.root_uri;
            if let Ok(root_path) = root_uri.to_file_path() {
                let config_path = root_path.join(".vscode").join("knot.json");
                if config_path.exists() {
                    drop(inner);
                    let mut inner = self.inner.write().await;
                    if let Ok(config_text) = std::fs::read_to_string(&config_path) {
                        if let Err(e) = inner.workspace.load_config(&config_text) {
                            tracing::warn!("Failed to reload knot.json on config change: {}", e);
                        } else {
                            tracing::info!("Reloaded .vscode/knot.json after configuration change");
                        }
                    }
                }
            }
        }

        // Fetch VS Code diagnostic settings via workspace/configuration
        let diag_keys: [(&str, &str); 13] = [
            ("BrokenLink", "broken-link"),
            ("UnreachablePassage", "unreachable-passage"),
            ("InfiniteLoop", "infinite-loop"),
            ("UninitializedVariable", "uninitialized-variable"),
            ("UnusedVariable", "unused-variable"),
            ("RedundantWrite", "redundant-write"),
            ("DuplicatePassageName", "duplicate-passage-name"),
            ("EmptyPassage", "empty-passage"),
            ("DeadEndPassage", "dead-end-passage"),
            ("InvalidPassageName", "invalid-passage-name"),
            ("OrphanedPassage", "orphaned-passage"),
            ("ComplexPassage", "complex-passage"),
            ("LargePassage", "large-passage"),
        ];

        let config_items: Vec<ConfigurationItem> = diag_keys
            .iter()
            .map(|(_, setting_name)| ConfigurationItem {
                scope_uri: None,
                section: Some(format!("knot.diagnostics.{}", setting_name)),
            })
            .collect();

        let config_values = self
            .client
            .configuration(config_items)
            .await
            .unwrap_or_default();

        // Apply VS Code diagnostic settings (they override knot.json defaults)
        let mut inner = self.inner.write().await;
        for (i, (diag_key, _)) in diag_keys.iter().enumerate() {
            if let Some(value) = config_values.get(i) {
                if let Some(severity_str) = value.as_str() {
                    let severity = match severity_str {
                        "error" => Some(knot_core::workspace::DiagnosticSeverity::Error),
                        "warning" => Some(knot_core::workspace::DiagnosticSeverity::Warning),
                        "info" => Some(knot_core::workspace::DiagnosticSeverity::Info),
                        "hint" => Some(knot_core::workspace::DiagnosticSeverity::Hint),
                        "off" => Some(knot_core::workspace::DiagnosticSeverity::Off),
                        _ => None,
                    };
                    if let Some(sev) = severity {
                        inner.workspace.config.diagnostics.insert(diag_key.to_string(), sev);
                    }
                }
            }
        }

        // Re-run analysis and publish diagnostics with updated config
        let diagnostics = AnalysisEngine::analyze(&inner.workspace);
        let open_docs = inner.open_documents.clone();
        let fmt_diags = inner.format_diagnostics.clone();
        let config = inner.workspace.config.clone();
        drop(inner);

        publish_all_diagnostics(&self.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
    }

    // -----------------------------------------------------------------------
    // File watcher
    // -----------------------------------------------------------------------

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        tracing::info!("did_change_watched_files: {} events", params.changes.len());

        for event in params.changes {
            let uri = event.uri;
            let file_type = uri.to_file_path().and_then(|p| {
                p.extension()
                    .and_then(|e| e.to_str().map(|s| s.to_string()))
                    .ok_or(())
            });

            let is_twee = match file_type.as_deref() {
                Ok("tw") | Ok("twee") => true,
                _ => false,
            };

            if !is_twee {
                continue;
            }

            match event.typ {
                FileChangeType::CREATED => {
                    tracing::info!("File created: {}", uri);
                    // Read and index the new file
                    if let Ok(path) = uri.to_file_path() {
                        if let Ok(text) = std::fs::read_to_string(&path) {
                            let mut inner = self.inner.write().await;
                            let format = inner.workspace.resolve_format();
                            let (doc, parse_result) =
                                parse_with_format_plugin(&inner.format_registry, &uri, &text, format, 0);

                            inner.open_documents.insert(uri.clone(), text.clone());
                            inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);
                            extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                            inner.workspace.insert_document(doc);
                            rebuild_graph(&mut inner.workspace);

                            let diagnostics = AnalysisEngine::analyze(&inner.workspace);
                            let open_docs = inner.open_documents.clone();
                            let fmt_diags = inner.format_diagnostics.clone();
                            let config = inner.workspace.config.clone();
                            drop(inner);

                            publish_all_diagnostics(&self.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
                        }
                    }
                }
                FileChangeType::DELETED => {
                    tracing::info!("File deleted: {}", uri);
                    let mut inner = self.inner.write().await;
                    inner.open_documents.remove(&uri);
                    inner.format_diagnostics.remove(&uri);
                    inner.workspace.remove_document_and_update_graph(&uri);

                    // Recheck broken links after removal
                    inner.workspace.graph.recheck_broken_links();

                    let diagnostics = AnalysisEngine::analyze(&inner.workspace);
                    let open_docs = inner.open_documents.clone();
                    let fmt_diags = inner.format_diagnostics.clone();
                    let config = inner.workspace.config.clone();
                    drop(inner);

                    publish_all_diagnostics(&self.client, &diagnostics, &fmt_diags, &open_docs, &config).await;

                    // Clear diagnostics for the deleted file
                    self.client
                        .publish_diagnostics(uri, Vec::new(), None)
                        .await;
                }
                FileChangeType::CHANGED => {
                    tracing::info!("File changed on disk: {}", uri);
                    // Re-read and re-index the file if it's not currently open
                    // (open files are tracked by did_change)
                    let is_open = {
                        let inner = self.inner.read().await;
                        inner.open_documents.contains_key(&uri)
                    };

                    if !is_open {
                        if let Ok(path) = uri.to_file_path() {
                            if let Ok(text) = std::fs::read_to_string(&path) {
                                let mut inner = self.inner.write().await;
                                let format = inner.workspace.resolve_format();
                                let (doc, parse_result) =
                                    parse_with_format_plugin(&inner.format_registry, &uri, &text, format, 0);

                                inner.open_documents.insert(uri.clone(), text.clone());
                                inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);
                                extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                                inner.workspace.insert_document(doc);
                                rebuild_graph(&mut inner.workspace);

                                let diagnostics = AnalysisEngine::analyze(&inner.workspace);
                                let open_docs = inner.open_documents.clone();
                                let fmt_diags = inner.format_diagnostics.clone();
                                let config = inner.workspace.config.clone();
                                drop(inner);

                                publish_all_diagnostics(&self.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    // -----------------------------------------------------------------------
    // Language features
    // -----------------------------------------------------------------------

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> Result<Option<CompletionResponse>, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        // Determine the trigger character
        let trigger = params.context.as_ref().and_then(|ctx| ctx.trigger_character.clone());

        let text = match inner.open_documents.get(uri) {
            Some(t) => t,
            None => return Ok(None),
        };

        let format = inner.workspace.resolve_format();
        let mut items: Vec<CompletionItem> = Vec::new();

        match trigger.as_deref() {
            Some("[") => {
                // Passage link completion — offer snippet [[${1:passage}]]
                let names = inner.workspace.all_passage_names();
                for (i, name) in names.iter().enumerate() {
                    // Find which passages link here for detail
                    let written_in = find_passages_linking_to(&inner.workspace, name);
                    let detail_str = if written_in.is_empty() {
                        "Passage".to_string()
                    } else if written_in.len() <= 3 {
                        format!("Passage — linked from {}", written_in.join(", "))
                    } else {
                        format!("Passage — linked from {} passages", written_in.len())
                    };
                    items.push(CompletionItem {
                        label: name.clone(),
                        kind: Some(CompletionItemKind::MODULE),
                        detail: Some(detail_str),
                        sort_text: Some(format!("0_{:06}", i)),
                        filter_text: Some(name.clone()),
                        insert_text: Some(format!("[[{}]]", name)),
                        insert_text_format: Some(InsertTextFormat::SNIPPET),
                        data: Some(serde_json::json!({"type": "passage", "name": name})),
                        ..Default::default()
                    });
                }
            }
            Some("$") => {
                // Variable completion from workspace
                let mut var_info: HashMap<String, Vec<String>> = HashMap::new();
                for doc in inner.workspace.documents() {
                    for passage in &doc.passages {
                        for var in &passage.vars {
                            if var.is_temporary { continue; }
                            var_info
                                .entry(var.name.clone())
                                .or_default()
                                .push(passage.name.clone());
                        }
                    }
                }
                let mut sorted_vars: Vec<_> = var_info.iter().collect();
                sorted_vars.sort_by(|a, b| a.0.cmp(b.0));
                for (i, (var_name, passages)) in sorted_vars.iter().enumerate() {
                    let detail_str = if passages.len() <= 3 {
                        format!("Variable — {}", passages.join(", "))
                    } else {
                        format!("Variable — {} passages", passages.len())
                    };
                    items.push(CompletionItem {
                        label: (*var_name).clone(),
                        kind: Some(CompletionItemKind::VARIABLE),
                        detail: Some(detail_str),
                        sort_text: Some(format!("1_{:06}", i)),
                        filter_text: Some(var_name.trim_start_matches('$').to_string()),
                        insert_text: Some(var_name.to_string()),
                        insert_text_format: Some(InsertTextFormat::SNIPPET),
                        data: Some(serde_json::json!({"type": "variable", "name": var_name})),
                        ..Default::default()
                    });
                }
            }
            Some("<") => {
                // SugarCube macro completion
                if matches!(format, StoryFormat::SugarCube) {
                    let macros = sugarcube_macro_signatures();
                    for (i, m) in macros.iter().enumerate() {
                        items.push(CompletionItem {
                            label: m.name.to_string(),
                            kind: Some(CompletionItemKind::FUNCTION),
                            detail: Some(format!("<<{} {}>>", m.name, m.signature)),
                            sort_text: Some(format!("2_{:06}", i)),
                            filter_text: Some(m.name.to_string()),
                            insert_text: Some(format!("<<{}{}>>", m.name, m.insertSnippet())),
                            insert_text_format: Some(InsertTextFormat::SNIPPET),
                            data: Some(serde_json::json!({"type": "macro", "name": m.name})),
                            ..Default::default()
                        });
                    }
                }
            }
            _ => {
                // Default: just passage names
                let names = inner.workspace.all_passage_names();
                for (i, name) in names.iter().enumerate() {
                    items.push(CompletionItem {
                        label: name.clone(),
                        kind: Some(CompletionItemKind::MODULE),
                        detail: Some("Passage".to_string()),
                        sort_text: Some(format!("0_{:06}", i)),
                        data: Some(serde_json::json!({"type": "passage", "name": name})),
                        ..Default::default()
                    });
                }
            }
        }

        if items.is_empty() {
            Ok(None)
        } else {
            Ok(Some(CompletionResponse::Array(items)))
        }
    }

    async fn completion_resolve(
        &self,
        params: CompletionItem,
    ) -> Result<CompletionItem, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        if let Some(data) = &params.data {
            let comp_type = data.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");

            match comp_type {
                "passage" => {
                    if let Some((doc, passage)) = inner.workspace.find_passage(name) {
                        let links_count = passage.links.len();
                        let incoming = count_incoming_links(&inner.workspace, name);
                        let doc_markdown = format!(
                            "**{}**\n\nFile: {}\nLinks out: {} | Incoming: {} | Tags: {}",
                            name,
                            doc.uri.as_str(),
                            links_count,
                            incoming,
                            if passage.tags.is_empty() { "none".to_string() } else { passage.tags.join(", ") }
                        );
                        return Ok(CompletionItem {
                            documentation: Some(Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: doc_markdown,
                            })),
                            ..params
                        });
                    }
                }
                "variable" => {
                    let doc_markdown = format!("**{}**\n\nStory variable (persistent across passages)", name);
                    return Ok(CompletionItem {
                        documentation: Some(Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: doc_markdown,
                        })),
                        ..params
                    });
                }
                "macro" => {
                    if let Some(sig) = sugarcube_macro_signatures().iter().find(|m| m.name == name) {
                        let doc_markdown = format!(
                            "**<<{} {}>>**\n\n{}",
                            sig.name, sig.signature, sig.description
                        );
                        return Ok(CompletionItem {
                            documentation: Some(Documentation::MarkupContent(MarkupContent {
                                kind: MarkupKind::Markdown,
                                value: doc_markdown,
                            })),
                            ..params
                        });
                    }
                }
                _ => {}
            }
        }

        Ok(params)
    }

    async fn hover(
        &self,
        params: HoverParams,
    ) -> Result<Option<Hover>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let inner = self.inner.read().await;

        if let Some(text) = inner.open_documents.get(uri) {
            if let Some(passage_name) = find_passage_at_position(text, position) {
                if let Some((_, passage)) = inner.workspace.find_passage(&passage_name) {
                    let links_count = passage.links.len();
                    let vars_count = passage.vars.len();
                    let tags = if passage.tags.is_empty() {
                        "none".to_string()
                    } else {
                        passage.tags.join(", ")
                    };

                    // Count incoming links (other passages that link to this one)
                    let incoming = count_incoming_links(&inner.workspace, &passage_name);

                    let hover_text = format!(
                        "**{}**\n\nLinks: {} | Variables: {} | Tags: {} | Incoming: {}",
                        passage.name, links_count, vars_count, tags, incoming
                    );
                    return Ok(Some(Hover {
                        contents: HoverContents::Markup(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: hover_text,
                        }),
                        range: None,
                    }));
                }
            }
        }

        Ok(None)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let inner = self.inner.read().await;

        if let Some(text) = inner.open_documents.get(uri) {
            if let Some(target_name) = find_link_target_at_position(text, position) {
                if let Some((doc, passage)) = inner.workspace.find_passage(&target_name) {
                    let target_uri = doc.uri.clone();
                    // Find the passage header line in the target document.
                    let target_text = inner.open_documents.get(&target_uri);
                    let range = if let Some(t) = target_text {
                        find_passage_header_range(t, &passage.name)
                    } else {
                        Range::default()
                    };

                    return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                        uri: target_uri,
                        range,
                    })));
                }
            }
        }

        Ok(None)
    }

    async fn references(
        &self,
        params: ReferenceParams,
    ) -> Result<Option<Vec<Location>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;

        let inner = self.inner.read().await;

        // First, determine what the user is on: a passage header or a link
        let target_passage = if let Some(text) = inner.open_documents.get(uri) {
            // Check if cursor is on a passage header
            if let Some(name) = find_passage_at_position(text, position) {
                Some(name)
            } else if let Some(name) = find_link_target_at_position(text, position) {
                // Cursor is on a link — find all references to the link target
                Some(name)
            } else {
                None
            }
        } else {
            None
        };

        let Some(target_name) = target_passage else {
            return Ok(None);
        };

        // Find all locations that reference this passage (links + definition)
        let mut locations = Vec::new();

        for (doc_uri, text) in &inner.open_documents {
            for (line_idx, line) in text.lines().enumerate() {
                // Check for passage header definition
                if line.starts_with("::") {
                    let name = parse_passage_name_from_header(&line[2..]);
                    if name == target_name {
                        locations.push(Location {
                            uri: doc_uri.clone(),
                            range: Range {
                                start: Position {
                                    line: line_idx as u32,
                                    character: 0,
                                },
                                end: Position {
                                    line: line_idx as u32,
                                    character: line.len() as u32,
                                },
                            },
                        });
                    }
                }

                // Check for links to this passage
                let mut search_from = 0;
                while let Some(rel_start) = line[search_from..].find("[[") {
                    let abs_start = search_from + rel_start;
                    if let Some(rel_end) = line[abs_start..].find("]]") {
                        let content_start = abs_start + 2;
                        let content_end = abs_start + rel_end;
                        let link_text = &line[content_start..content_end];

                        let link_target = if let Some(arrow) = link_text.find("->") {
                            &link_text[arrow + 2..]
                        } else if let Some(pipe) = link_text.find('|') {
                            &link_text[pipe + 1..]
                        } else {
                            link_text
                        };

                        if link_target.trim() == target_name {
                            locations.push(Location {
                                uri: doc_uri.clone(),
                                range: Range {
                                    start: Position {
                                        line: line_idx as u32,
                                        character: content_start as u32,
                                    },
                                    end: Position {
                                        line: line_idx as u32,
                                        character: content_end as u32,
                                    },
                                },
                            });
                        }

                        search_from = abs_start + rel_end + 2;
                    } else {
                        break;
                    }
                }
            }
        }

        if locations.is_empty() {
            Ok(None)
        } else {
            Ok(Some(locations))
        }
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let mut symbols = Vec::new();

        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let name = parse_passage_name_from_header(&line[2..]);

                // Find the end of this passage (next :: or end of file)
                let end_line = text
                    .lines()
                    .enumerate()
                    .skip(line_idx + 1)
                    .find(|(_, l)| l.starts_with("::"))
                    .map(|(i, _)| i as u32 - 1)
                    .unwrap_or_else(|| text.lines().count() as u32 - 1);

                let kind = if name == "StoryData" || name == "StoryTitle" {
                    SymbolKind::CONSTANT
                } else {
                    SymbolKind::MODULE
                };

                // Extract tags from the header line if present
                let detail = if let Some(bracket_start) = line[2..].find('[') {
                    let header = &line[2..];
                    if let Some(bracket_end) = header[bracket_start..].find(']') {
                        let tags = &header[bracket_start + 1..bracket_start + bracket_end];
                        Some(format!("Tags: {}", tags))
                    } else {
                        None
                    }
                } else {
                    None
                };

                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name,
                    detail,
                    kind,
                    tags: None,
                    deprecated: None,
                    range: Range {
                        start: Position {
                            line: line_idx as u32,
                            character: 0,
                        },
                        end: Position {
                            line: end_line,
                            character: 0,
                        },
                    },
                    selection_range: Range {
                        start: Position {
                            line: line_idx as u32,
                            character: 2, // after "::"
                        },
                        end: Position {
                            line: line_idx as u32,
                            character: line.len() as u32,
                        },
                    },
                    children: None,
                });
            }
        }

        if symbols.is_empty() {
            Ok(None)
        } else {
            Ok(Some(DocumentSymbolResponse::Nested(symbols)))
        }
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        if let Some(text) = inner.open_documents.get(uri) {
            let format = inner.workspace.resolve_format();
            if let Some(plugin) = inner.format_registry.get(&format) {
                let parse_result = plugin.parse(uri, text);
                let tokens = convert_semantic_tokens(text, &parse_result.tokens);
                let data = encode_semantic_tokens(&tokens);
                return Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
                    result_id: None,
                    data,
                })));
            }
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Declaration — same as definition for Twine (links to passage header)
    // -----------------------------------------------------------------------

    async fn goto_declaration(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
        self.goto_definition(params).await
    }

    // -----------------------------------------------------------------------
    // Implementation — show passages that link TO this passage
    // -----------------------------------------------------------------------

    async fn goto_implementation(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let inner = self.inner.read().await;

        let target_passage = if let Some(text) = inner.open_documents.get(uri) {
            find_passage_at_position(text, position)
                .or_else(|| find_link_target_at_position(text, position))
        } else {
            None
        };

        let Some(target_name) = target_passage else {
            return Ok(None);
        };

        // Find all passages that link TO this passage
        let mut locations = Vec::new();
        for (doc_uri, text) in &inner.open_documents {
            for (line_idx, line) in text.lines().enumerate() {
                let mut search_from = 0;
                while let Some(rel_start) = line[search_from..].find("[[") {
                    let abs_start = search_from + rel_start;
                    if let Some(rel_end) = line[abs_start..].find("]]") {
                        let content_start = abs_start + 2;
                        let content_end = abs_start + rel_end;
                        let link_text = &line[content_start..content_end];
                        let link_target = if let Some(arrow) = link_text.find("->") {
                            &link_text[arrow + 2..]
                        } else if let Some(pipe) = link_text.find('|') {
                            &link_text[pipe + 1..]
                        } else {
                            link_text
                        };
                        if link_target.trim() == target_name {
                            locations.push(Location {
                                uri: doc_uri.clone(),
                                range: Range {
                                    start: Position { line: line_idx as u32, character: content_start as u32 },
                                    end: Position { line: line_idx as u32, character: content_end as u32 },
                                },
                            });
                        }
                        search_from = abs_start + rel_end + 2;
                    } else {
                        break;
                    }
                }
            }
        }

        if locations.is_empty() {
            Ok(None)
        } else {
            Ok(Some(GotoDefinitionResponse::Array(locations)))
        }
    }

    // -----------------------------------------------------------------------
    // Type Definition — go to the StoryData passage
    // -----------------------------------------------------------------------

    async fn goto_type_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Find the StoryData passage in the workspace
        if let Some((doc, _passage)) = inner.workspace.find_passage("StoryData") {
            let target_uri = doc.uri.clone();
            let target_text = inner.open_documents.get(&target_uri);
            let range = if let Some(t) = target_text {
                find_passage_header_range(t, "StoryData")
            } else {
                Range::default()
            };
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: target_uri,
                range,
            })));
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Rename + Prepare Rename
    // -----------------------------------------------------------------------

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let position = params.position;

        let inner = self.inner.read().await;

        if let Some(text) = inner.open_documents.get(uri) {
            // Check if cursor is on a passage header
            if let Some(name) = find_passage_at_position(text, position) {
                let line_text = text.lines().nth(position.line as usize).unwrap_or("");
                let name_start = line_text.find(&name).unwrap_or(2) as u32;
                let name_end = name_start + name.len() as u32;
                return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                    range: Range {
                        start: Position { line: position.line, character: name_start },
                        end: Position { line: position.line, character: name_end },
                    },
                    placeholder: name,
                }));
            }

            // Check if cursor is on a link target
            if let Some(target_name) = find_link_target_at_position(text, position) {
                let line_text = text.lines().nth(position.line as usize).unwrap_or("");
                // Find the [[...]] that contains the cursor
                let mut search_from = 0;
                while let Some(rel_start) = line_text[search_from..].find("[[") {
                    let abs_start = search_from + rel_start;
                    if let Some(rel_end) = line_text[abs_start..].find("]]") {
                        let content_start = abs_start + 2;
                        let content_end = abs_start + rel_end;
                        if position.character as usize >= content_start && position.character as usize <= content_end {
                            return Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
                                range: Range {
                                    start: Position { line: position.line, character: content_start as u32 },
                                    end: Position { line: position.line, character: content_end as u32 },
                                },
                                placeholder: target_name,
                            }));
                        }
                        search_from = abs_start + rel_end + 2;
                    } else {
                        break;
                    }
                }
            }
        }

        Ok(None)
    }

    async fn rename(
        &self,
        params: RenameParams,
    ) -> Result<Option<WorkspaceEdit>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let new_name = params.new_name;

        let inner = self.inner.read().await;

        // Determine what the user is renaming
        let target_passage = if let Some(text) = inner.open_documents.get(uri) {
            find_passage_at_position(text, position)
                .or_else(|| find_link_target_at_position(text, position))
        } else {
            None
        };

        let Some(old_name) = target_passage else {
            return Ok(None);
        };

        // Collect all edits across all documents
        let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

        for (doc_uri, text) in &inner.open_documents {
            let mut doc_edits = Vec::new();

            for (line_idx, line) in text.lines().enumerate() {
                // Rename passage header
                if line.starts_with("::") {
                    let name = parse_passage_name_from_header(&line[2..]);
                    if name == old_name {
                        let name_start = line.find(&name).unwrap_or(2);
                        doc_edits.push(TextEdit {
                            range: Range {
                                start: Position { line: line_idx as u32, character: name_start as u32 },
                                end: Position { line: line_idx as u32, character: (name_start + name.len()) as u32 },
                            },
                            new_text: new_name.clone(),
                        });
                    }
                }

                // Rename links
                let mut search_from = 0;
                while let Some(rel_start) = line[search_from..].find("[[") {
                    let abs_start = search_from + rel_start;
                    if let Some(rel_end) = line[abs_start..].find("]]") {
                        let content_start = abs_start + 2;
                        let content_end = abs_start + rel_end;
                        let link_text = &line[content_start..content_end];

                        let link_target = if let Some(arrow) = link_text.find("->") {
                            &link_text[arrow + 2..]
                        } else if let Some(pipe) = link_text.find('|') {
                            &link_text[pipe + 1..]
                        } else {
                            link_text
                        };

                        if link_target.trim() == old_name {
                            // Find the exact position of the target name in the link
                            let target_start = content_start + (link_text.len() - link_target.len());
                            doc_edits.push(TextEdit {
                                range: Range {
                                    start: Position { line: line_idx as u32, character: target_start as u32 },
                                    end: Position { line: line_idx as u32, character: (target_start + link_target.trim().len()) as u32 },
                                },
                                new_text: new_name.clone(),
                            });
                        }

                        search_from = abs_start + rel_end + 2;
                    } else {
                        break;
                    }
                }
            }

            if !doc_edits.is_empty() {
                changes.insert(doc_uri.clone(), doc_edits);
            }
        }

        if changes.is_empty() {
            Ok(None)
        } else {
            Ok(Some(WorkspaceEdit {
                changes: Some(changes),
                document_changes: None,
                change_annotations: None,
            }))
        }
    }

    // -----------------------------------------------------------------------
    // Workspace Symbol
    // -----------------------------------------------------------------------

    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> Result<Option<Vec<SymbolInformation>>, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;
        let query = params.query.to_lowercase();

        let mut symbols = Vec::new();

        for doc in inner.workspace.documents() {
            let text = match inner.open_documents.get(&doc.uri) {
                Some(t) => t,
                None => continue,
            };

            for (line_idx, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let name = parse_passage_name_from_header(&line[2..]);

                    // Filter by query (case-insensitive substring match)
                    if !query.is_empty() && !name.to_lowercase().contains(&query) {
                        continue;
                    }

                    let kind = if name == "StoryData" || name == "StoryTitle" {
                        SymbolKind::CONSTANT
                    } else {
                        SymbolKind::MODULE
                    };

                    symbols.push(SymbolInformation {
                        name,
                        kind,
                        tags: None,
                        deprecated: None,
                        location: Location {
                            uri: doc.uri.clone(),
                            range: Range {
                                start: Position { line: line_idx as u32, character: 0 },
                                end: Position { line: line_idx as u32, character: line.len() as u32 },
                            },
                        },
                        container_name: None,
                    });
                }
            }
        }

        if symbols.is_empty() {
            Ok(None)
        } else {
            Ok(Some(symbols))
        }
    }

    // -----------------------------------------------------------------------
    // Signature Help
    // -----------------------------------------------------------------------

    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let inner = self.inner.read().await;
        let format = inner.workspace.resolve_format();

        if !matches!(format, StoryFormat::SugarCube) {
            return Ok(None);
        }

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        // Find if cursor is inside a <<macro ...>> construct
        let line_text = match text.lines().nth(position.line as usize) {
            Some(l) => l,
            None => return Ok(None),
        };

        let mut search_from = 0;
        while let Some(rel_start) = line_text[search_from..].find("<<") {
            let abs_start = search_from + rel_start;
            if let Some(rel_end) = line_text[abs_start..].find(">>") {
                let content_start = abs_start + 2;
                let content_end = abs_start + rel_end;
                let char_pos = position.character as usize;

                if char_pos >= content_start && char_pos <= content_end {
                    let macro_content = &line_text[content_start..content_end];
                    let macro_name = macro_content.split_whitespace().next().unwrap_or("");

                    if let Some(sig) = sugarcube_macro_signatures().iter().find(|m| m.name == macro_name) {
                        // Count commas to determine active parameter
                        let after_name = &macro_content[macro_name.len()..];
                        let active_param = after_name.matches(',').count() as u32;

                        let params_list: Vec<ParameterInformation> = sig
                            .param_names()
                            .iter()
                            .map(|p| ParameterInformation {
                                label: ParameterLabel::Simple(p.clone()),
                                documentation: None,
                            })
                            .collect();

                        return Ok(Some(SignatureHelp {
                            signatures: vec![SignatureInformation {
                                label: format!("<<{} {}>>", sig.name, sig.signature),
                                documentation: Some(Documentation::MarkupContent(MarkupContent {
                                    kind: MarkupKind::Markdown,
                                    value: sig.description.to_string(),
                                })),
                                parameters: if params_list.is_empty() { None } else { Some(params_list) },
                                active_parameter: if sig.param_names().is_empty() { None } else { Some(active_param) },
                            }],
                            active_signature: Some(0),
                            active_parameter: if sig.param_names().is_empty() { None } else { Some(active_param) },
                        }));
                    }
                }

                search_from = abs_start + rel_end + 2;
            } else {
                // Unclosed macro — cursor might be inside
                let content_start = abs_start + 2;
                let char_pos = position.character as usize;
                if char_pos >= content_start {
                    let macro_content = &line_text[content_start..];
                    let macro_name = macro_content.split_whitespace().next().unwrap_or("");

                    if let Some(sig) = sugarcube_macro_signatures().iter().find(|m| m.name == macro_name) {
                        let after_name = &macro_content[macro_name.len()..];
                        let active_param = after_name.matches(',').count() as u32;

                        let params_list: Vec<ParameterInformation> = sig
                            .param_names()
                            .iter()
                            .map(|p| ParameterInformation {
                                label: ParameterLabel::Simple(p.clone()),
                                documentation: None,
                            })
                            .collect();

                        return Ok(Some(SignatureHelp {
                            signatures: vec![SignatureInformation {
                                label: format!("<<{} {}>>", sig.name, sig.signature),
                                documentation: Some(Documentation::MarkupContent(MarkupContent {
                                    kind: MarkupKind::Markdown,
                                    value: sig.description.to_string(),
                                })),
                                parameters: if params_list.is_empty() { None } else { Some(params_list) },
                                active_parameter: if sig.param_names().is_empty() { None } else { Some(active_param) },
                            }],
                            active_signature: Some(0),
                            active_parameter: if sig.param_names().is_empty() { None } else { Some(active_param) },
                        }));
                    }
                }
                break;
            }
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Code Actions
    // -----------------------------------------------------------------------

    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> Result<Option<CodeActionResponse>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let mut actions: Vec<CodeActionOrCommand> = Vec::new();

        for diag in &params.context.diagnostics {
            let code = match &diag.code {
                Some(NumberOrString::String(s)) => s.clone(),
                _ => continue,
            };

            match code.as_str() {
                "BrokenLink" => {
                    // Extract the broken link target from the message
                    if let Some(name) = extract_quoted_name(&diag.message) {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: format!("Create passage '{}'", name),
                            kind: Some(CodeActionKind::QUICKFIX),
                            diagnostics: Some(vec![diag.clone()]),
                            edit: Some(create_passage_edit(&inner, &name)),
                            is_preferred: Some(true),
                            ..Default::default()
                        }));
                    }
                }
                "UnreachablePassage" => {
                    if let Some(name) = extract_passage_from_diag(&diag.message) {
                        // Find nearest reachable passage
                        let nearest = find_nearest_reachable_passage(&inner.workspace, &name);
                        if let Some(near) = nearest {
                            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                                title: format!("Add link from '{}' to '{}'", near, name),
                                kind: Some(CodeActionKind::QUICKFIX),
                                diagnostics: Some(vec![diag.clone()]),
                                edit: Some(add_link_edit(&inner, &near, &name)),
                                ..Default::default()
                            }));
                        }
                    }
                }
                "DuplicatePassageName" => {
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: "Rename passage".to_string(),
                        kind: Some(CodeActionKind::REFACTOR),
                        diagnostics: Some(vec![diag.clone()]),
                        ..Default::default()
                    }));
                }
                "EmptyPassage" => {
                    if let Some(name) = extract_passage_from_diag(&diag.message) {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: format!("Add content template to '{}'", name),
                            kind: Some(CodeActionKind::QUICKFIX),
                            diagnostics: Some(vec![diag.clone()]),
                            edit: Some(add_content_template_edit(&inner, &name)),
                            ..Default::default()
                        }));
                    }
                }
                "UninitializedVariable" => {
                    if let Some(var_name) = extract_variable_name(&diag.message) {
                        actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                            title: format!("Initialize {} in StoryInit", var_name),
                            kind: Some(CodeActionKind::QUICKFIX),
                            diagnostics: Some(vec![diag.clone()]),
                            edit: Some(initialize_var_in_story_init_edit(&inner, &var_name)),
                            is_preferred: Some(true),
                            ..Default::default()
                        }));
                    }
                }
                _ => {}
            }
        }

        if actions.is_empty() {
            Ok(None)
        } else {
            Ok(Some(actions))
        }
    }

    // -----------------------------------------------------------------------
    // Code Lens
    // -----------------------------------------------------------------------

    async fn code_lens(
        &self,
        params: CodeLensParams,
    ) -> Result<Option<Vec<CodeLens>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let mut lenses = Vec::new();

        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let name = parse_passage_name_from_header(&line[2..]);
                let outgoing = inner.workspace.graph.outgoing_neighbors(&name).len();
                let incoming = count_incoming_links(&inner.workspace, &name);

                if outgoing > 0 || incoming > 0 {
                    lenses.push(CodeLens {
                        range: Range {
                            start: Position { line: line_idx as u32, character: 0 },
                            end: Position { line: line_idx as u32, character: line.len() as u32 },
                        },
                        command: Some(Command {
                            title: if outgoing > 0 {
                                format!("{} links →", outgoing)
                            } else {
                                format!("{} refs", incoming)
                            },
                            command: String::new(),
                            arguments: None,
                        }),
                        data: None,
                    });
                }
            }
        }

        if lenses.is_empty() {
            Ok(None)
        } else {
            Ok(Some(lenses))
        }
    }

    // -----------------------------------------------------------------------
    // Inlay Hints
    // -----------------------------------------------------------------------

    async fn inlay_hint(
        &self,
        params: InlayHintParams,
    ) -> Result<Option<Vec<InlayHint>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let start_passage = inner
            .workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        let passage_data = AnalysisEngine::collect_passage_data(&inner.workspace);
        let seed_init = AnalysisEngine::collect_special_passage_initializers(&inner.workspace, &passage_data);
        let flow_states = AnalysisEngine::run_dataflow_from_engine(&inner.workspace, start_passage, &passage_data, &seed_init);

        let mut hints = Vec::new();

        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let name = parse_passage_name_from_header(&line[2..]);

                if let Some(state) = flow_states.get(&name) {
                    let mut init_vars: Vec<&String> = state.entry.iter().collect();
                    init_vars.sort();

                    if !init_vars.is_empty() {
                        let label = format!("// initialized: {}", init_vars.iter().map(|v| v.as_str()).collect::<Vec<_>>().join(", "));
                        hints.push(InlayHint {
                            position: Position { line: line_idx as u32, character: 0 },
                            label: InlayHintLabel::String(label),
                            kind: Some(InlayHintKind::TYPE),
                            text_edits: None,
                            tooltip: None,
                            padding_left: Some(true),
                            padding_right: Some(true),
                            data: None,
                        });
                    }

                    // Check for potentially uninitialized variables
                    let mut local_init = state.entry.clone();
                    let mut uninit_vars = Vec::new();
                    if let Some((_, passage)) = inner.workspace.find_passage(&name) {
                        for var in passage.vars_sorted_by_span() {
                            if var.is_temporary { continue; }
                            match var.kind {
                                knot_core::passage::VarKind::Read => {
                                    if !local_init.contains(&var.name) {
                                        if !uninit_vars.contains(&var.name) {
                                            uninit_vars.push(var.name.clone());
                                        }
                                    }
                                }
                                knot_core::passage::VarKind::Write => {
                                    local_init.insert(var.name.clone());
                                }
                            }
                        }
                    }

                    if !uninit_vars.is_empty() {
                        let label = format!("// may be uninitialized: {}", uninit_vars.join(", "));
                        hints.push(InlayHint {
                            position: Position { line: line_idx as u32, character: 0 },
                            label: InlayHintLabel::String(label),
                            kind: Some(InlayHintKind::PARAMETER),
                            text_edits: None,
                            tooltip: None,
                            padding_left: Some(true),
                            padding_right: Some(true),
                            data: None,
                        });
                    }
                }
            }
        }

        if hints.is_empty() {
            Ok(None)
        } else {
            Ok(Some(hints))
        }
    }

    // -----------------------------------------------------------------------
    // Folding Range
    // -----------------------------------------------------------------------

    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> Result<Option<Vec<FoldingRange>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let mut ranges = Vec::new();
        let lines: Vec<&str> = text.lines().collect();

        for (line_idx, line) in lines.iter().enumerate() {
            if line.starts_with("::") {
                // Find the end of this passage (next :: or end of file)
                let end_line = lines[line_idx + 1..]
                    .iter()
                    .position(|l| l.starts_with("::"))
                    .map(|i| line_idx + 1 + i)
                    .unwrap_or(lines.len());

                if end_line > line_idx + 1 {
                    ranges.push(FoldingRange {
                        start_line: (line_idx + 1) as u32,
                        start_character: None,
                        end_line: (end_line - 1) as u32,
                        end_character: None,
                        kind: Some(FoldingRangeKind::Region),
                        collapsed_text: None,
                    });
                }
            }
        }

        if ranges.is_empty() {
            Ok(None)
        } else {
            Ok(Some(ranges))
        }
    }

    // -----------------------------------------------------------------------
    // Document Link
    // -----------------------------------------------------------------------

    async fn document_link(
        &self,
        params: DocumentLinkParams,
    ) -> Result<Option<Vec<DocumentLink>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let mut links = Vec::new();

        for (line_idx, line) in text.lines().enumerate() {
            let mut search_from = 0;
            while let Some(rel_start) = line[search_from..].find("[[") {
                let abs_start = search_from + rel_start;
                if let Some(rel_end) = line[abs_start..].find("]]") {
                    let content_start = abs_start + 2;
                    let content_end = abs_start + rel_end;
                    let link_text = &line[content_start..content_end];

                    let target = if let Some(arrow) = link_text.find("->") {
                        &link_text[arrow + 2..]
                    } else if let Some(pipe) = link_text.find('|') {
                        &link_text[pipe + 1..]
                    } else {
                        link_text
                    };
                    let target = target.trim();

                    if !target.is_empty() {
                        // Find the target passage's URI
                        if let Some(target_uri) = inner.workspace.find_passage_file_uri(target) {
                            links.push(DocumentLink {
                                range: Range {
                                    start: Position { line: line_idx as u32, character: content_start as u32 },
                                    end: Position { line: line_idx as u32, character: content_end as u32 },
                                },
                                target: Some(target_uri),
                                tooltip: Some(format!("Go to {}", target)),
                                data: None,
                            });
                        }
                    }

                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
                }
            }
        }

        if links.is_empty() {
            Ok(None)
        } else {
            Ok(Some(links))
        }
    }

    // -----------------------------------------------------------------------
    // Selection Range
    // -----------------------------------------------------------------------

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let mut results = Vec::new();

        for position in &params.positions {
            let mut range_chain: Vec<Range> = Vec::new();

            // Level 1: Link text (if inside a [[...]])
            if let Some(target) = find_link_target_at_position(text, *position) {
                let line_text = text.lines().nth(position.line as usize).unwrap_or("");
                let mut search_from = 0;
                while let Some(rel_start) = line_text[search_from..].find("[[") {
                    let abs_start = search_from + rel_start;
                    if let Some(rel_end) = line_text[abs_start..].find("]]") {
                        let content_start = abs_start + 2;
                        let content_end = abs_start + rel_end;
                        if position.character as usize >= content_start && position.character as usize <= content_end {
                            // Link text range
                            let target_start = if let Some(arrow) = line_text[content_start..content_end].find("->") {
                                content_start + arrow + 2
                            } else if let Some(pipe) = line_text[content_start..content_end].find('|') {
                                content_start + pipe + 1
                            } else {
                                content_start
                            };
                            range_chain.push(Range {
                                start: Position { line: position.line, character: target_start as u32 },
                                end: Position { line: position.line, character: content_end as u32 },
                            });
                            // Full link range
                            range_chain.push(Range {
                                start: Position { line: position.line, character: abs_start as u32 },
                                end: Position { line: position.line, character: (abs_start + rel_end + 2) as u32 },
                            });
                            break;
                        }
                        search_from = abs_start + rel_end + 2;
                    } else {
                        break;
                    }
                }
            }

            // Level 2: Passage body range
            if let Some(name) = find_passage_at_position(text, *position) {
                let header_line = position.line;
                let lines: Vec<&str> = text.lines().collect();
                let end_line = lines[(header_line as usize) + 1..]
                    .iter()
                    .position(|l| l.starts_with("::"))
                    .map(|i| header_line + 1 + i as u32)
                    .unwrap_or(lines.len() as u32 - 1);

                range_chain.push(Range {
                    start: Position { line: header_line + 1, character: 0 },
                    end: Position { line: end_line, character: 0 },
                });

                // Level 3: Passage header + body
                range_chain.push(Range {
                    start: Position { line: header_line, character: 0 },
                    end: Position { line: end_line, character: 0 },
                });

                let _ = name; // used above
            }

            // Build the linked SelectionRange list (innermost first)
            let sel_range = range_chain.into_iter().rev().fold(None::<SelectionRange>, |parent, range| {
                Some(SelectionRange {
                    range,
                    parent: parent.map(Box::new),
                })
            });

            results.push(sel_range.unwrap_or(SelectionRange {
                range: Range {
                    start: *position,
                    end: Position { line: position.line, character: position.character + 1 },
                },
                parent: None,
            }));
        }

        Ok(Some(results))
    }

    // -----------------------------------------------------------------------
    // Formatting
    // -----------------------------------------------------------------------

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let edits = format_twee_text(text);
        if edits.is_empty() {
            Ok(None)
        } else {
            Ok(Some(edits))
        }
    }

    // -----------------------------------------------------------------------
    // Range Formatting
    // -----------------------------------------------------------------------

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let all_edits = format_twee_text(text);
        // Filter edits to those within the requested range
        let range = params.range;
        let filtered: Vec<TextEdit> = all_edits
            .into_iter()
            .filter(|edit| {
                edit.range.start.line >= range.start.line
                    && edit.range.end.line <= range.end.line
            })
            .collect();

        if filtered.is_empty() {
            Ok(None)
        } else {
            Ok(Some(filtered))
        }
    }

    // -----------------------------------------------------------------------
    // On-Type Formatting
    // -----------------------------------------------------------------------

    async fn on_type_formatting(
        &self,
        params: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position.text_document.uri;
        let position = params.text_document_position.position;
        let ch = &params.ch;

        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let line_text = text.lines().nth(position.line as usize).unwrap_or("");
        let char_pos = position.character as usize;

        // Auto-close [[ with ]]
        if ch == "]" && char_pos >= 2 {
            let before = &line_text[..char_pos];
            if before.ends_with("[[") {
                let insert_pos = Position { line: position.line, character: char_pos as u32 };
                return Ok(Some(vec![TextEdit {
                    range: Range { start: insert_pos, end: insert_pos },
                    new_text: "]]".to_string(),
                }]));
            }
        }

        // Auto-close << with >>
        if ch == ">" && char_pos >= 2 {
            let before = &line_text[..char_pos];
            if before.ends_with("<<") {
                let insert_pos = Position { line: position.line, character: char_pos as u32 };
                return Ok(Some(vec![TextEdit {
                    range: Range { start: insert_pos, end: insert_pos },
                    new_text: ">>".to_string(),
                }]));
            }
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Linked Editing Range
    // -----------------------------------------------------------------------

    async fn linked_editing_range(
        &self,
        params: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        // If cursor is on a passage header name, find all [[link]] references
        if let Some(name) = find_passage_at_position(text, position) {
            let line_text = text.lines().nth(position.line as usize).unwrap_or("");
            let name_start = line_text.find(&name).unwrap_or(2);

            let mut ranges = vec![Range {
                start: Position { line: position.line, character: name_start as u32 },
                end: Position { line: position.line, character: (name_start + name.len()) as u32 },
            }];

            // Find all [[name]] links in the document
            for (line_idx, line) in text.lines().enumerate() {
                let mut search_from = 0;
                while let Some(rel_start) = line[search_from..].find("[[") {
                    let abs_start = search_from + rel_start;
                    if let Some(rel_end) = line[abs_start..].find("]]") {
                        let content_start = abs_start + 2;
                        let content_end = abs_start + rel_end;
                        let link_text = &line[content_start..content_end];

                        let link_target = if let Some(arrow) = link_text.find("->") {
                            &link_text[arrow + 2..]
                        } else if let Some(pipe) = link_text.find('|') {
                            &link_text[pipe + 1..]
                        } else {
                            link_text
                        };

                        if link_target.trim() == name {
                            let target_start = content_start + (link_text.len() - link_target.len());
                            ranges.push(Range {
                                start: Position { line: line_idx as u32, character: target_start as u32 },
                                end: Position { line: line_idx as u32, character: (target_start + name.len()) as u32 },
                            });
                        }

                        search_from = abs_start + rel_end + 2;
                    } else {
                        break;
                    }
                }
            }

            return Ok(Some(LinkedEditingRanges {
                ranges,
                word_pattern: None,
            }));
        }

        Ok(None)
    }

    // -----------------------------------------------------------------------
    // Call Hierarchy
    // -----------------------------------------------------------------------

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let inner = self.inner.read().await;

        let Some(text) = inner.open_documents.get(uri) else {
            return Ok(None);
        };

        let target_passage = find_passage_at_position(text, position)
            .or_else(|| find_link_target_at_position(text, position));

        let Some(name) = target_passage else {
            return Ok(None);
        };

        // Find the passage definition location
        if let Some((doc, _passage)) = inner.workspace.find_passage(&name) {
            let target_uri = doc.uri.clone();
            let target_text = inner.open_documents.get(&target_uri);
            let range = if let Some(t) = target_text {
                find_passage_header_range(t, &name)
            } else {
                Range::default()
            };

            return Ok(Some(vec![CallHierarchyItem {
                name,
                kind: SymbolKind::MODULE,
                tags: None,
                detail: None,
                uri: target_uri,
                range,
                selection_range: range,
                data: None,
            }]));
        }

        Ok(None)
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>, tower_lsp::jsonrpc::Error> {
        let item = &params.item;
        let name = &item.name;

        let inner = self.inner.read().await;

        let mut calls = Vec::new();

        // Find all passages that link TO this passage
        for (doc_uri, text) in &inner.open_documents {
            for (line_idx, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let source_name = parse_passage_name_from_header(&line[2..]);
                    let mut search_from = 0;
                    while let Some(rel_start) = line[search_from..].find("[[") {
                        let abs_start = search_from + rel_start;
                        if let Some(rel_end) = line[abs_start..].find("]]") {
                            let content_start = abs_start + 2;
                            let content_end = abs_start + rel_end;
                            let link_text = &line[content_start..content_end];

                            let link_target = if let Some(arrow) = link_text.find("->") {
                                &link_text[arrow + 2..]
                            } else if let Some(pipe) = link_text.find('|') {
                                &link_text[pipe + 1..]
                            } else {
                                link_text
                            };

                            if link_target.trim() == *name {
                                let source_range = find_passage_header_range(text, &source_name);
                                calls.push(CallHierarchyIncomingCall {
                                    from: CallHierarchyItem {
                                        name: source_name,
                                        kind: SymbolKind::MODULE,
                                        tags: None,
                                        detail: None,
                                        uri: doc_uri.clone(),
                                        range: source_range,
                                        selection_range: source_range,
                                        data: None,
                                    },
                                    from_ranges: vec![Range {
                                        start: Position { line: line_idx as u32, character: content_start as u32 },
                                        end: Position { line: line_idx as u32, character: content_end as u32 },
                                    }],
                                });
                                break; // One match per passage is enough
                            }
                            search_from = abs_start + rel_end + 2;
                        } else {
                            break;
                        }
                    }
                }
            }
        }

        if calls.is_empty() {
            Ok(None)
        } else {
            Ok(Some(calls))
        }
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>, tower_lsp::jsonrpc::Error> {
        let item = &params.item;
        let name = &item.name;

        let inner = self.inner.read().await;

        let mut calls = Vec::new();

        // Find the passage and its outgoing links
        if let Some((doc, passage)) = inner.workspace.find_passage(name) {
            let text = inner.open_documents.get(&doc.uri);
            for link in &passage.links {
                if let Some((target_doc, _target_passage)) = inner.workspace.find_passage(&link.target) {
                    let target_uri = target_doc.uri.clone();
                    let target_text = inner.open_documents.get(&target_uri);
                    let target_range = if let Some(t) = target_text {
                        find_passage_header_range(t, &link.target)
                    } else {
                        Range::default()
                    };

                    // Find the link range in the source
                    let from_ranges = if let Some(t) = text {
                        find_link_ranges_for_target(t, &link.target)
                    } else {
                        vec![]
                    };

                    calls.push(CallHierarchyOutgoingCall {
                        to: CallHierarchyItem {
                            name: link.target.clone(),
                            kind: SymbolKind::MODULE,
                            tags: None,
                            detail: None,
                            uri: target_uri,
                            range: target_range,
                            selection_range: target_range,
                            data: None,
                        },
                        from_ranges,
                    });
                }
            }
        }

        if calls.is_empty() {
            Ok(None)
        } else {
            Ok(Some(calls))
        }
    }

    // -----------------------------------------------------------------------
    // Pull Diagnostics
    // -----------------------------------------------------------------------

    async fn diagnostic(
        &self,
        params: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReportResult, tower_lsp::jsonrpc::Error> {
        let uri = &params.text_document.uri;
        let inner = self.inner.read().await;

        let text = match inner.open_documents.get(uri) {
            Some(t) => t,
            None => {
                return Ok(DocumentDiagnosticReportResult::Report(
                    DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                        related_documents: None,
                        full_document_diagnostic_report: FullDocumentDiagnosticReport {
                            result_id: None,
                            items: vec![],
                        },
                    }),
                ));
            }
        };

        let diagnostics = AnalysisEngine::analyze(&inner.workspace);
        let uri_str = uri.to_string();
        let config = &inner.workspace.config;

        let mut items: Vec<Diagnostic> = Vec::new();

        for gd in &diagnostics {
            if gd.file_uri != uri_str { continue; }

            let default_severity = match gd.kind {
                DiagnosticKind::BrokenLink => DiagnosticSeverity::WARNING,
                DiagnosticKind::UnreachablePassage => DiagnosticSeverity::HINT,
                DiagnosticKind::InfiniteLoop => DiagnosticSeverity::WARNING,
                DiagnosticKind::UninitializedVariable => DiagnosticSeverity::WARNING,
                DiagnosticKind::UnusedVariable => DiagnosticSeverity::HINT,
                DiagnosticKind::RedundantWrite => DiagnosticSeverity::HINT,
                DiagnosticKind::DuplicateStoryData => DiagnosticSeverity::ERROR,
                DiagnosticKind::MissingStoryData => DiagnosticSeverity::WARNING,
                DiagnosticKind::MissingStartPassage => DiagnosticSeverity::ERROR,
                DiagnosticKind::UnsupportedFormat => DiagnosticSeverity::ERROR,
                DiagnosticKind::DuplicatePassageName => DiagnosticSeverity::ERROR,
                DiagnosticKind::EmptyPassage => DiagnosticSeverity::HINT,
                DiagnosticKind::DeadEndPassage => DiagnosticSeverity::INFORMATION,
                DiagnosticKind::InvalidPassageName => DiagnosticSeverity::WARNING,
                DiagnosticKind::OrphanedPassage => DiagnosticSeverity::INFORMATION,
                DiagnosticKind::ComplexPassage => DiagnosticSeverity::HINT,
                DiagnosticKind::LargePassage => DiagnosticSeverity::HINT,
                DiagnosticKind::MissingStartLink => DiagnosticSeverity::WARNING,
            };

            let diag_key = format!("{:?}", gd.kind);
            let severity = if let Some(custom) = config.diagnostics.get(&diag_key) {
                match custom {
                    knot_core::workspace::DiagnosticSeverity::Off => continue,
                    knot_core::workspace::DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                    knot_core::workspace::DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                    knot_core::workspace::DiagnosticSeverity::Info => DiagnosticSeverity::INFORMATION,
                    knot_core::workspace::DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
                }
            } else {
                default_severity
            };

            let range = find_passage_header_range(text, &gd.passage_name);

            // Build related information
            let related_information = build_related_information(
                &inner, &gd.kind, &gd.passage_name, &gd.message,
            );

            items.push(Diagnostic {
                range,
                severity: Some(severity),
                code: Some(NumberOrString::String(diag_key)),
                source: Some("knot".to_string()),
                message: gd.message.clone(),
                related_information,
                ..Default::default()
            });
        }

        // Also add format diagnostics
        if let Some(fmt_diags) = inner.format_diagnostics.get(uri) {
            for fd in fmt_diags {
                let range = byte_range_to_lsp_range(text, &fd.range);
                let severity = match fd.severity {
                    fmt_plugin::FormatDiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                    fmt_plugin::FormatDiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                    fmt_plugin::FormatDiagnosticSeverity::Info => DiagnosticSeverity::INFORMATION,
                    fmt_plugin::FormatDiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
                };
                items.push(Diagnostic {
                    range,
                    severity: Some(severity),
                    code: Some(NumberOrString::String(format!("format:{}", fd.code))),
                    source: Some("knot".to_string()),
                    message: fd.message.clone(),
                    ..Default::default()
                });
            }
        }

        Ok(DocumentDiagnosticReportResult::Report(
            DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
                related_documents: None,
                full_document_diagnostic_report: FullDocumentDiagnosticReport {
                    result_id: None,
                    items,
                },
            }),
        ))
    }
}

// ---------------------------------------------------------------------------
// Custom LSP request handlers (knot/graph, knot/build, knot/play)
// ---------------------------------------------------------------------------

impl ServerState {
    /// `knot/graph` — export the passage graph for the Story Map webview.
    pub async fn knot_graph(
        &self,
        _params: KnotGraphParams,
    ) -> Result<KnotGraphResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;

        // Collect passage tags from all documents
        let mut passage_tags: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        let mut passage_lines: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();

        for doc in inner.workspace.documents() {
            for passage in &doc.passages {
                passage_tags.insert(passage.name.clone(), passage.tags.clone());
            }
            // Find passage line numbers from the document text
            if let Some(text) = inner.open_documents.get(&doc.uri) {
                let mut line_num: u32 = 0;
                for line in text.lines() {
                    if line.starts_with("::") {
                        let name = parse_passage_name_from_header(&line[2..]);
                        passage_lines.insert(name, line_num);
                    }
                    line_num += 1;
                }
            }
        }

        // Determine unreachable passages
        let start_passage = inner
            .workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");
        let unreachable_diags = inner.workspace.graph.detect_unreachable(start_passage);
        let unreachable_set: std::collections::HashSet<String> = unreachable_diags
            .iter()
            .map(|d| d.passage_name.clone())
            .collect();

        let export = inner.workspace.graph.export_graph_with_metadata(
            &passage_tags,
            &unreachable_set,
        );

        let nodes: Vec<KnotGraphNode> = export
            .nodes
            .into_iter()
            .map(|n| KnotGraphNode {
                id: n.id,
                label: n.label.clone(),
                file: n.file.clone(),
                line: passage_lines.get(&n.label).copied().unwrap_or(0),
                tags: n.tags,
                out_degree: n.out_degree,
                in_degree: n.in_degree,
                is_special: n.is_special,
                is_metadata: n.is_metadata,
                is_unreachable: unreachable_set.contains(&n.label),
            })
            .collect();

        let edges: Vec<KnotGraphEdge> = export
            .edges
            .into_iter()
            .map(|e| KnotGraphEdge {
                source: e.source,
                target: e.target,
                is_broken: e.is_broken,
            })
            .collect();

        Ok(KnotGraphResponse {
            nodes,
            edges,
            layout: Some("dagre".to_string()),
        })
    }

    /// `knot/build` — trigger project compilation.
    ///
    /// Detects and invokes the Tweego compiler (or configured alternative)
    /// to build the project into an HTML file. Build output is streamed
    /// to the client via `knot/buildOutput` notifications.
    pub async fn knot_build(
        &self,
        params: KnotBuildParams,
    ) -> Result<KnotBuildResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;
        let root_uri = inner.workspace.root_uri.clone();
        let config = inner.workspace.config.clone();
        drop(inner);

        let root_path = match root_uri.to_file_path() {
            Ok(p) => p,
            Err(_) => {
                return Ok(KnotBuildResponse {
                    success: false,
                    output_path: None,
                    errors: vec!["Workspace root is not a valid file path".to_string()],
                });
            }
        };

        // Resolve compiler path: config override > PATH lookup
        let compiler_path = if let Some(ref path) = config.compiler_path {
            Some(path.clone())
        } else {
            which_compiler()
        };

        let Some(compiler_path) = compiler_path else {
            return Ok(KnotBuildResponse {
                success: false,
                output_path: None,
                errors: vec![
                    "No Twine compiler found. Install Tweego and ensure it is on PATH, or set compiler_path in .vscode/knot.json".to_string()
                ],
            });
        };

        // Determine output directory
        let output_dir = root_path.join(&config.build.output_dir);
        std::fs::create_dir_all(&output_dir).ok();

        let output_file = output_dir.join("index.html");

        // Build the command arguments
        let mut args: Vec<String> = Vec::new();

        // If a start passage is specified, add --start flag before the output argument
        if let Some(ref start_passage) = params.start_passage {
            args.push("--start".to_string());
            args.push(start_passage.clone());
        }

        args.push("-o".to_string());
        args.push(output_file.to_string_lossy().to_string());
        args.extend(config.build.flags.iter().cloned());
        args.push(root_path.to_string_lossy().to_string());

        tracing::info!("Build command: {} {}", compiler_path.display(), args.join(" "));

        // Run the compiler
        let output = tokio::process::Command::new(&compiler_path)
            .args(&args)
            .current_dir(&root_path)
            .output()
            .await;

        match output {
            Ok(output) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                // Stream build output to the client
                for line in stdout.lines() {
                    self.client
                        .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                            line: line.to_string(),
                            is_error: false,
                        })
                        .await;
                }
                for line in stderr.lines() {
                    self.client
                        .send_notification::<KnotBuildOutputNotification>(KnotBuildOutput {
                            line: line.to_string(),
                            is_error: true,
                        })
                        .await;
                }

                if output.status.success() {
                    tracing::info!("Build succeeded: {}", output_file.display());
                    Ok(KnotBuildResponse {
                        success: true,
                        output_path: Some(output_file.to_string_lossy().to_string()),
                        errors: Vec::new(),
                    })
                } else {
                    let error_lines: Vec<String> = stderr.lines().map(|l| l.to_string()).collect();
                    tracing::warn!("Build failed: {}", error_lines.join("; "));
                    Ok(KnotBuildResponse {
                        success: false,
                        output_path: None,
                        errors: if error_lines.is_empty() {
                            vec!["Build failed with no error output".to_string()]
                        } else {
                            error_lines
                        },
                    })
                }
            }
            Err(e) => {
                tracing::error!("Failed to execute compiler: {}", e);
                Ok(KnotBuildResponse {
                    success: false,
                    output_path: None,
                    errors: vec![format!("Failed to execute compiler: {}", e)],
                })
            }
        }
    }

    /// `knot/play` — compile the project and return the HTML path for preview.
    ///
    /// This first triggers a build, then returns the path to the compiled
    /// HTML file so the VS Code extension can load it in a webview.
    pub async fn knot_play(
        &self,
        params: KnotPlayParams,
    ) -> Result<KnotPlayResponse, tower_lsp::jsonrpc::Error> {
        // Build first
        let build_result = self.knot_build(KnotBuildParams {
            workspace_uri: params.workspace_uri.clone(),
            start_passage: params.start_passage.clone(),
        }).await?;

        if build_result.success {
            Ok(KnotPlayResponse {
                html_path: build_result.output_path,
                error: None,
            })
        } else {
            Ok(KnotPlayResponse {
                html_path: None,
                error: Some(build_result.errors.join("\n")),
            })
        }
    }

    /// `knot/variableFlow` — export variable dataflow information.
    ///
    /// Returns per-variable usage information including which passages
    /// write and read each variable, whether it is initialized at start,
    /// and whether it is unused.
    pub async fn knot_variable_flow(
        &self,
        params: KnotVariableFlowParams,
    ) -> Result<KnotVariableFlowResponse, tower_lsp::jsonrpc::Error> {
        use crate::lsp_ext::{KnotVariableFlowResponse, KnotVariableInfo, KnotVariableLocation};

        let inner = self.inner.read().await;
        let workspace = &inner.workspace;
        let format = workspace.resolve_format();

        // Only provide variable flow for formats that support it
        if !format.supports_full_variable_tracking() && !format.supports_partial_variable_tracking() {
            return Ok(KnotVariableFlowResponse {
                variables: Vec::new(),
            });
        }

        // Collect variable usage across all passages
        let mut var_map: HashMap<String, KnotVariableInfo> = HashMap::new();

        // First, collect initializer variables from special passages
        let _start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        // Run dataflow to determine which variables are initialized at start
        let passage_data = AnalysisEngine::collect_passage_data(workspace);
        let seed_init = AnalysisEngine::collect_special_passage_initializers(workspace, &passage_data);

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }

                for var in &passage.vars {
                    if var.is_temporary {
                        continue;
                    }

                    // Apply optional filter
                    if let Some(ref filter) = params.variable_name {
                        if var.name != *filter {
                            continue;
                        }
                    }

                    let entry = var_map.entry(var.name.clone()).or_insert_with(|| KnotVariableInfo {
                        name: var.name.clone(),
                        is_temporary: false,
                        written_in: Vec::new(),
                        read_in: Vec::new(),
                        initialized_at_start: seed_init.contains(&var.name),
                        is_unused: false,
                    });

                    let location = KnotVariableLocation {
                        passage_name: passage.name.clone(),
                        file_uri: doc.uri.to_string(),
                        is_write: var.kind == knot_core::VarKind::Write,
                    };

                    if var.kind == knot_core::VarKind::Write {
                        entry.written_in.push(location);
                    } else {
                        entry.read_in.push(location);
                    }
                }
            }
        }

        // Determine which variables are unused (written but never read)
        for info in var_map.values_mut() {
            info.is_unused = !info.written_in.is_empty() && info.read_in.is_empty();
        }

        let mut variables: Vec<KnotVariableInfo> = var_map.into_values().collect();
        // Sort by name for deterministic output
        variables.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(KnotVariableFlowResponse {
            variables,
        })
    }

    /// `knot/debug` — return debug information about a specific passage.
    ///
    /// Provides detailed information about a passage's state including
    /// variable operations, link connections, reachability, and diagnostics.
    pub async fn knot_debug(
        &self,
        params: KnotDebugParams,
    ) -> Result<KnotDebugResponse, tower_lsp::jsonrpc::Error> {
        use crate::lsp_ext::{KnotDebugDiagnostic, KnotDebugLink, KnotDebugResponse, KnotDebugVariable};

        let inner = self.inner.read().await;
        let workspace = &inner.workspace;

        // Find the passage
        let (doc, passage) = match workspace.find_passage(&params.passage_name) {
            Some(result) => result,
            None => {
                let name = params.passage_name.clone();
                return Ok(KnotDebugResponse {
                    passage_name: params.passage_name,
                    file_uri: String::new(),
                    is_reachable: false,
                    is_special: false,
                    is_metadata: false,
                    variables_written: Vec::new(),
                    variables_read: Vec::new(),
                    initialized_at_entry: Vec::new(),
                    outgoing_links: Vec::new(),
                    incoming_links: Vec::new(),
                    predecessors: Vec::new(),
                    successors: Vec::new(),
                    in_infinite_loop: false,
                    diagnostics: vec![KnotDebugDiagnostic {
                        kind: "NotFound".to_string(),
                        message: format!("Passage '{}' not found in workspace", name),
                    }],
                });
            }
        };

        let file_uri = doc.uri.to_string();

        // Determine reachability
        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");
        let unreachable_diags = workspace.graph.detect_unreachable(start_passage);
        let is_reachable = !unreachable_diags.iter().any(|d| d.passage_name == params.passage_name);

        // Variable info
        let variables_written: Vec<KnotDebugVariable> = passage
            .persistent_variable_writes()
            .map(|v| KnotDebugVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
            })
            .collect();

        let variables_read: Vec<KnotDebugVariable> = passage
            .persistent_variable_reads()
            .map(|v| KnotDebugVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
            })
            .collect();

        // Compute initialized-at-entry from dataflow
        let passage_data = AnalysisEngine::collect_passage_data(workspace);
        let seed_init = AnalysisEngine::collect_special_passage_initializers(workspace, &passage_data);
        let flow_states = AnalysisEngine::run_dataflow_from_engine(workspace, start_passage, &passage_data, &seed_init);
        let initialized_at_entry: Vec<String> = flow_states
            .get(&params.passage_name)
            .map(|s| {
                let mut vars: Vec<String> = s.entry.iter().cloned().collect();
                vars.sort();
                vars
            })
            .unwrap_or_default();

        // Outgoing links
        let outgoing_links: Vec<KnotDebugLink> = passage
            .links
            .iter()
            .map(|l| KnotDebugLink {
                passage_name: l.target.clone(),
                display_text: l.display_text.clone(),
                target_exists: workspace.find_passage(&l.target).is_some(),
            })
            .collect();

        // Incoming links
        let mut incoming_links = Vec::new();
        for other_doc in workspace.documents() {
            for other_passage in &other_doc.passages {
                for link in &other_passage.links {
                    if link.target == params.passage_name {
                        incoming_links.push(KnotDebugLink {
                            passage_name: other_passage.name.clone(),
                            display_text: link.display_text.clone(),
                            target_exists: true, // The target is this passage, which we know exists
                        });
                    }
                }
            }
        }

        // Graph neighbors
        let predecessors = workspace.graph.incoming_neighbors(&params.passage_name);
        let successors = workspace.graph.outgoing_neighbors(&params.passage_name);

        // Check if in infinite loop
        let passage_vars: std::collections::HashMap<String, Vec<&knot_core::passage::VarOp>> =
            AnalysisEngine::collect_passage_vars_as_ref(workspace);
        let loop_diags = workspace.graph.detect_infinite_loops(&passage_vars);
        let in_infinite_loop = loop_diags.iter().any(|d| d.passage_name == params.passage_name);

        // Diagnostics for this passage
        let all_diagnostics = AnalysisEngine::analyze(workspace);
        let diagnostics: Vec<KnotDebugDiagnostic> = all_diagnostics
            .iter()
            .filter(|d| d.passage_name == params.passage_name)
            .map(|d| KnotDebugDiagnostic {
                kind: format!("{:?}", d.kind),
                message: d.message.clone(),
            })
            .collect();

        Ok(KnotDebugResponse {
            passage_name: params.passage_name,
            file_uri,
            is_reachable,
            is_special: passage.is_special,
            is_metadata: passage.is_metadata(),
            variables_written,
            variables_read,
            initialized_at_entry,
            outgoing_links,
            incoming_links,
            predecessors,
            successors,
            in_infinite_loop,
            diagnostics,
        })
    }

    /// `knot/trace` — simulate execution from a given passage.
    ///
    /// Performs a DFS traversal of the passage graph starting from the
    /// given passage, recording each step including available choices,
    /// variable mutations, and loop detection.
    pub async fn knot_trace(
        &self,
        params: KnotTraceParams,
    ) -> Result<KnotTraceResponse, tower_lsp::jsonrpc::Error> {
        use crate::lsp_ext::{KnotTraceResponse, KnotTraceStep};

        let inner = self.inner.read().await;
        let workspace = &inner.workspace;

        let mut steps = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut truncated = false;

        // DFS trace
        let mut stack: Vec<(String, u32)> = vec![(params.start_passage.clone(), 0)];
        let mut on_stack = std::collections::HashSet::new();

        while let Some((passage_name, depth)) = stack.pop() {
            if depth > params.max_depth {
                truncated = true;
                continue;
            }

            let is_loop = on_stack.contains(&passage_name);

            if visited.contains(&passage_name) && !is_loop {
                // Already fully processed, skip (but record loop info)
                if is_loop {
                    steps.push(KnotTraceStep {
                        passage_name: passage_name.clone(),
                        depth,
                        variables_written: Vec::new(),
                        available_links: Vec::new(),
                        is_loop: true,
                    });
                }
                continue;
            }

            // Find the passage data
            let (variables_written, available_links) = if let Some((_, passage)) = workspace.find_passage(&passage_name) {
                let vars: Vec<String> = passage.persistent_variable_writes().map(|v| v.name.clone()).collect();
                let links: Vec<String> = passage.links.iter().map(|l| l.target.clone()).collect();
                (vars, links)
            } else {
                (Vec::new(), Vec::new())
            };

            steps.push(KnotTraceStep {
                passage_name: passage_name.clone(),
                depth,
                variables_written,
                available_links: available_links.clone(),
                is_loop,
            });

            visited.insert(passage_name.clone());
            on_stack.insert(passage_name.clone());

            // Add successors to the stack (in reverse order so first link is processed first)
            for target in available_links.into_iter().rev() {
                if !visited.contains(&target) || on_stack.contains(&target) {
                    stack.push((target, depth + 1));
                }
            }

            on_stack.remove(&passage_name);
        }

        Ok(KnotTraceResponse {
            steps,
            truncated,
        })
    }

    /// `knot/profile` — return workspace profiling statistics.
    ///
    /// Computes and returns comprehensive workspace statistics including
    /// passage counts, link density, variable metrics, and graph analysis results.
    pub async fn knot_profile(
        &self,
        _params: KnotProfileParams,
    ) -> Result<KnotProfileResponse, tower_lsp::jsonrpc::Error> {
        use crate::lsp_ext::{KnotComplexityMetrics, KnotLinkDistribution, KnotProfileResponse, KnotStructuralBalance, KnotTagStat};

        let inner = self.inner.read().await;
        let workspace = &inner.workspace;

        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        // Basic counts
        let document_count = workspace.document_count() as u32;
        let passage_count = workspace.passage_count() as u32;

        let mut special_passage_count: u32 = 0;
        let mut metadata_passage_count: u32 = 0;
        let mut total_word_count: u32 = 0;
        let mut dead_end_count: u32 = 0;
        let mut orphaned_count: u32 = 0;

        let mut all_variables: std::collections::HashSet<String> = std::collections::HashSet::new();
        let variable_issue_count: u32;

        // Per-passage word counts for complexity metrics
        let mut passage_word_counts: Vec<u32> = Vec::new();
        let mut passage_out_links: Vec<u32> = Vec::new();
        // Per-tag statistics
        let mut tag_data: HashMap<String, (u32, u32, u32)> = HashMap::new(); // tag → (count, total_words, total_out_links)

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    metadata_passage_count += 1;
                }
                if passage.is_special {
                    special_passage_count += 1;
                }

                // Word count (approximate from text blocks)
                let passage_words: u32 = passage
                    .body
                    .iter()
                    .map(|block| match block {
                        knot_core::passage::Block::Text { content, .. } => content.split_whitespace().count() as u32,
                        _ => 0,
                    })
                    .sum();
                total_word_count += passage_words;

                if !passage.is_metadata() {
                    passage_word_counts.push(passage_words);

                    let out_count = workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                    passage_out_links.push(out_count);

                    // Dead-end detection
                    if !passage.is_special {
                        let has_outgoing = !passage.links.is_empty() || out_count > 0;
                        if !has_outgoing {
                            dead_end_count += 1;
                        }
                    }

                    // Orphan detection (1 incoming link)
                    let in_count = workspace.graph.incoming_neighbors(&passage.name).len();
                    if in_count == 1 && !passage.is_special {
                        orphaned_count += 1;
                    }
                }

                // Per-tag statistics
                for tag in &passage.tags {
                    let entry = tag_data.entry(tag.clone()).or_insert((0, 0, 0));
                    entry.0 += 1;
                    entry.1 += passage_words;
                    entry.2 += workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                }

                // Variable collection
                for var in &passage.vars {
                    if !var.is_temporary {
                        all_variables.insert(var.name.clone());
                    }
                }
            }
        }

        // Build tag stats
        let tag_stats: Vec<KnotTagStat> = tag_data
            .into_iter()
            .map(|(tag, (count, total_words, total_out))| KnotTagStat {
                tag,
                passage_count: count,
                avg_word_count: if count > 0 { total_words as f64 / count as f64 } else { 0.0 },
                total_word_count: total_words,
                avg_out_links: if count > 0 { total_out as f64 / count as f64 } else { 0.0 },
            })
            .collect();

        // Complexity metrics
        let avg_word_count = if !passage_word_counts.is_empty() {
            passage_word_counts.iter().sum::<u32>() as f64 / passage_word_counts.len() as f64
        } else {
            0.0
        };

        let mut sorted_words = passage_word_counts.clone();
        sorted_words.sort();
        let median_word_count = if !sorted_words.is_empty() {
            let mid = sorted_words.len() / 2;
            if sorted_words.len() % 2 == 0 {
                (sorted_words[mid - 1] + sorted_words[mid]) as f64 / 2.0
            } else {
                sorted_words[mid] as f64
            }
        } else {
            0.0
        };

        let max_word_count = passage_word_counts.iter().max().copied().unwrap_or(0);
        let min_word_count = passage_word_counts.iter().filter(|&&w| w > 0).min().copied().unwrap_or(0);

        let avg_out_links = if !passage_out_links.is_empty() {
            let sum: u32 = passage_out_links.iter().sum();
            sum as f64 / passage_out_links.len() as f64
        } else {
            0.0
        };

        let out_links_stddev = if passage_out_links.len() > 1 {
            let mean = avg_out_links;
            let variance: f64 = passage_out_links
                .iter()
                .map(|&x| (x as f64 - mean).powi(2))
                .sum::<f64>()
                / (passage_out_links.len() - 1) as f64;
            variance.sqrt()
        } else {
            0.0
        };

        let complex_passage_count = passage_out_links.iter().filter(|&&x| x > 6).count() as u32;

        // Graph analysis
        let diagnostics = AnalysisEngine::analyze(workspace);
        let unreachable_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::UnreachablePassage)).count() as u32;
        let broken_link_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::BrokenLink)).count() as u32;
        let infinite_loop_count = diagnostics.iter().filter(|d| matches!(d.kind, knot_core::graph::DiagnosticKind::InfiniteLoop)).count() as u32;
        variable_issue_count = diagnostics.iter().filter(|d| matches!(
            d.kind,
            knot_core::graph::DiagnosticKind::UninitializedVariable
            | knot_core::graph::DiagnosticKind::UnusedVariable
            | knot_core::graph::DiagnosticKind::RedundantWrite
        )).count() as u32;

        let total_links = workspace.graph.edge_count() as u32;
        let graph_passage_count = workspace.graph.passage_count() as u32;

        let avg_out_degree = if graph_passage_count > 0 {
            total_links as f64 / graph_passage_count as f64
        } else {
            0.0
        };

        let avg_in_degree = avg_out_degree;

        // Compute max depth from start passage using BFS
        let max_depth = compute_max_depth(workspace, start_passage);

        // Link distribution
        let mut zero_links: u32 = 0;
        let mut few_links: u32 = 0;
        let mut moderate_links: u32 = 0;
        let mut many_links: u32 = 0;

        for doc in workspace.documents() {
            for passage in &doc.passages {
                if passage.is_metadata() {
                    continue;
                }
                let out_count = workspace.graph.outgoing_neighbors(&passage.name).len() as u32;
                match out_count {
                    0 => zero_links += 1,
                    1..=2 => few_links += 1,
                    3..=5 => moderate_links += 1,
                    _ => many_links += 1,
                }
            }
        }

        let format = workspace.resolve_format();
        let format_version = workspace.metadata.as_ref().and_then(|m| m.format_version.clone());
        let has_story_data = workspace.metadata.is_some();

        // Structural balance analysis
        let non_meta_count = if passage_count > metadata_passage_count {
            passage_count - metadata_passage_count
        } else {
            1
        };

        let dead_end_ratio = dead_end_count as f64 / non_meta_count as f64;
        let orphaned_ratio = orphaned_count as f64 / non_meta_count as f64;

        // Compute connected components using graph
        let connected_components = compute_connected_components(workspace);
        let is_well_connected = connected_components <= 1;

        // Compute diameter (longest shortest path from start)
        let diameter = max_depth; // Simplified — uses max depth as approximation

        // Average clustering coefficient (simplified — count passages that link to
        // passages that also link to each other)
        let avg_clustering = compute_avg_clustering(workspace);

        Ok(KnotProfileResponse {
            document_count,
            passage_count,
            special_passage_count,
            metadata_passage_count,
            unreachable_passage_count: unreachable_count,
            broken_link_count,
            infinite_loop_count,
            total_links,
            avg_out_degree,
            avg_in_degree,
            max_depth,
            dead_end_count,
            variable_count: all_variables.len() as u32,
            variable_issue_count,
            format: format.to_string(),
            format_version,
            has_story_data,
            total_word_count,
            link_distribution: KnotLinkDistribution {
                zero_links,
                few_links,
                moderate_links,
                many_links,
            },
            tag_stats,
            complexity_metrics: KnotComplexityMetrics {
                avg_word_count,
                median_word_count,
                max_word_count,
                min_word_count,
                avg_out_links,
                out_links_stddev,
                complex_passage_count,
            },
            structural_balance: KnotStructuralBalance {
                dead_end_ratio,
                orphaned_ratio,
                is_well_connected,
                connected_components,
                diameter,
                avg_clustering,
            },
        })
    }

    /// `knot/compilerDetect` — detect whether a Twine compiler is available.
    ///
    /// Checks for Tweego on PATH and in the configured compiler_path.
    pub async fn knot_compiler_detect(
        &self,
        _params: KnotCompilerDetectParams,
    ) -> Result<KnotCompilerDetectResponse, tower_lsp::jsonrpc::Error> {
        let inner = self.inner.read().await;
        let config = inner.workspace.config.clone();
        drop(inner);

        // Check configured path first
        if let Some(ref path) = config.compiler_path {
            if path.exists() {
                return Ok(KnotCompilerDetectResponse {
                    compiler_found: true,
                    compiler_name: Some("tweego".to_string()),
                    compiler_version: detect_compiler_version(path).await,
                    compiler_path: Some(path.to_string_lossy().to_string()),
                });
            }
        }

        // Check PATH
        if let Some(path) = which_compiler() {
            return Ok(KnotCompilerDetectResponse {
                compiler_found: true,
                compiler_name: Some("tweego".to_string()),
                compiler_version: detect_compiler_version(&path).await,
                compiler_path: Some(path.to_string_lossy().to_string()),
            });
        }

        Ok(KnotCompilerDetectResponse {
            compiler_found: false,
            compiler_name: None,
            compiler_version: None,
            compiler_path: None,
        })
    }

    /// `knot/breakpoints` — manage debug breakpoints on passages.
    ///
    /// Supports setting, clearing, and listing breakpoints. Breakpoints
    /// mark passages where execution should pause during trace/step debugging.
    pub async fn knot_breakpoints(
        &self,
        params: KnotBreakpointsParams,
    ) -> Result<KnotBreakpointsResponse, tower_lsp::jsonrpc::Error> {
        use crate::lsp_ext::{KnotBreakpointInfo, KnotBreakpointsResponse};

        // Handle mutations
        if params.clear_all == Some(true) {
            let mut inner = self.inner.write().await;
            inner.breakpoints.clear();
        } else if let Some(new_breakpoints) = params.set_breakpoints {
            let mut inner = self.inner.write().await;
            inner.breakpoints = new_breakpoints;
        }

        // Build response with current breakpoint state
        let inner = self.inner.read().await;
        let workspace = &inner.workspace;

        let breakpoints: Vec<KnotBreakpointInfo> = inner
            .breakpoints
            .iter()
            .map(|name| {
                let (passage_exists, file_uri, incoming, outgoing) =
                    if let Some((doc, passage)) = workspace.find_passage(name) {
                        (
                            true,
                            Some(doc.uri.to_string()),
                            workspace.graph.incoming_neighbors(name).len() as u32,
                            passage.links.len() as u32,
                        )
                    } else {
                        (false, None, 0, 0)
                    };

                KnotBreakpointInfo {
                    passage_name: name.clone(),
                    passage_exists,
                    file_uri,
                    incoming_links: incoming,
                    outgoing_links: outgoing,
                }
            })
            .collect();

        Ok(KnotBreakpointsResponse { breakpoints })
    }

    /// `knot/stepOver` — simulate a single step from a passage.
    ///
    /// Returns the available choices (outgoing links) from a passage,
    /// along with variable operations performed in that passage.
    pub async fn knot_step_over(
        &self,
        params: KnotStepOverParams,
    ) -> Result<KnotStepOverResponse, tower_lsp::jsonrpc::Error> {
        use crate::lsp_ext::{KnotStepChoice, KnotStepOverResponse};

        let inner = self.inner.read().await;
        let workspace = &inner.workspace;

        let (choices, variables_written, variables_read) =
            if let Some((_, passage)) = workspace.find_passage(&params.from_passage) {
                let choices: Vec<KnotStepChoice> = passage
                    .links
                    .iter()
                    .map(|l| KnotStepChoice {
                        passage_name: l.target.clone(),
                        display_text: l.display_text.clone(),
                        target_exists: workspace.find_passage(&l.target).is_some(),
                    })
                    .collect();

                let written: Vec<String> = passage
                    .persistent_variable_writes()
                    .map(|v| v.name.clone())
                    .collect();

                let read: Vec<String> = passage
                    .persistent_variable_reads()
                    .map(|v| v.name.clone())
                    .collect();

                (choices, written, read)
            } else {
                (Vec::new(), Vec::new(), Vec::new())
            };

        Ok(KnotStepOverResponse {
            from_passage: params.from_passage,
            choices,
            variables_written,
            variables_read,
        })
    }

    /// `knot/watchVariables` — get variable state at a specific passage.
    ///
    /// Returns detailed variable information for a passage including
    /// initialized-at-entry, written, read, and potentially uninitialized
    /// variables. This is used by the debug variable watch panel.
    pub async fn knot_watch_variables(
        &self,
        params: KnotWatchVariablesParams,
    ) -> Result<KnotWatchVariablesResponse, tower_lsp::jsonrpc::Error> {
        use crate::lsp_ext::{KnotWatchVariable, KnotWatchVariablesResponse};

        let inner = self.inner.read().await;
        let workspace = &inner.workspace;
        let format = workspace.resolve_format();

        // Only provide variable watch for formats that support it
        if !format.supports_full_variable_tracking() && !format.supports_partial_variable_tracking() {
            return Ok(KnotWatchVariablesResponse {
                at_passage: params.at_passage,
                initialized_at_entry: Vec::new(),
                written_in_passage: Vec::new(),
                read_in_passage: Vec::new(),
                potentially_uninitialized: Vec::new(),
            });
        }

        // Run dataflow analysis
        let start_passage = workspace
            .metadata
            .as_ref()
            .map(|m| m.start_passage.as_str())
            .unwrap_or("Start");

        let passage_data = AnalysisEngine::collect_passage_data(workspace);
        let seed_init = AnalysisEngine::collect_special_passage_initializers(workspace, &passage_data);
        let flow_states = AnalysisEngine::run_dataflow_from_engine(workspace, start_passage, &passage_data, &seed_init);

        // Get passage info
        let (doc_uri, passage) = match workspace.find_passage(&params.at_passage) {
            Some((doc, p)) => (doc.uri.to_string(), p),
            None => {
                return Ok(KnotWatchVariablesResponse {
                    at_passage: params.at_passage,
                    initialized_at_entry: Vec::new(),
                    written_in_passage: Vec::new(),
                    read_in_passage: Vec::new(),
                    potentially_uninitialized: Vec::new(),
                });
            }
        };

        let entry_init = flow_states
            .get(&params.at_passage)
            .map(|s| &s.entry)
            .cloned()
            .unwrap_or_default();

        // Apply filter if specified
        let filter_set: Option<std::collections::HashSet<String>> = params
            .filter
            .map(|f| f.into_iter().collect());

        // Build initialized-at-entry list
        let initialized_at_entry: Vec<KnotWatchVariable> = entry_init
            .iter()
            .filter(|v| {
                filter_set.as_ref().map_or(true, |f| f.contains(*v))
            })
            .map(|v| KnotWatchVariable {
                name: v.clone(),
                is_temporary: false,
                file_uri: doc_uri.clone(),
                last_written_in: None, // Could be enhanced with backward tracing
            })
            .collect();

        // Build written-in-passage list
        let written_in_passage: Vec<KnotWatchVariable> = passage
            .persistent_variable_writes()
            .filter(|v| {
                filter_set.as_ref().map_or(true, |f| f.contains(&v.name))
            })
            .map(|v| KnotWatchVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
                file_uri: doc_uri.clone(),
                last_written_in: Some(params.at_passage.clone()),
            })
            .collect();

        // Build read-in-passage list
        let read_in_passage: Vec<KnotWatchVariable> = passage
            .persistent_variable_reads()
            .filter(|v| {
                filter_set.as_ref().map_or(true, |f| f.contains(&v.name))
            })
            .map(|v| KnotWatchVariable {
                name: v.name.clone(),
                is_temporary: v.is_temporary,
                file_uri: doc_uri.clone(),
                last_written_in: None,
            })
            .collect();

        // Build potentially-uninitialized list
        let mut local_init = entry_init;
        let mut potentially_uninitialized = Vec::new();

        for var in passage.vars_sorted_by_span() {
            if var.is_temporary { continue; }
            if filter_set.as_ref().map_or(true, |f| f.contains(&var.name)) {
                match var.kind {
                    knot_core::passage::VarKind::Read => {
                        if !local_init.contains(&var.name) {
                            potentially_uninitialized.push(KnotWatchVariable {
                                name: var.name.clone(),
                                is_temporary: false,
                                file_uri: doc_uri.clone(),
                                last_written_in: None,
                            });
                        }
                    }
                    knot_core::passage::VarKind::Write => {
                        local_init.insert(var.name.clone());
                    }
                }
            }
        }

        Ok(KnotWatchVariablesResponse {
            at_passage: params.at_passage,
            initialized_at_entry,
            written_in_passage,
            read_in_passage,
            potentially_uninitialized,
        })
    }

    /// Register file watchers for .tw and .twee files using the
    /// `client/registerCapability` LSP request.
    async fn register_file_watchers(&self) {
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

        if let Err(e) = self.client.register_capability(registrations).await {
            tracing::warn!("Failed to register file watchers: {}", e);
        } else {
            tracing::info!("Registered file watchers for .tw/.twee files");
        }
    }
}

// ===========================================================================
// Format plugin parsing
// ===========================================================================

/// Parse a document using the format plugin system.
///
/// Returns both the constructed `Document` and the `ParseResult` (which
/// includes format-specific diagnostics and semantic tokens).
///
/// Falls back to the default format if the requested format plugin is not
/// available.
fn parse_with_format_plugin(
    registry: &fmt_plugin::FormatRegistry,
    uri: &Url,
    text: &str,
    format: StoryFormat,
    version: i32,
) -> (Document, fmt_plugin::ParseResult) {
    let plugin = registry
        .get(&format)
        .or_else(|| {
            // Try the default format
            let default = StoryFormat::default_format();
            registry.get(&default)
        });

    if let Some(plugin) = plugin {
        let result = plugin.parse(uri, text);
        let mut doc = Document::new(uri.clone(), format);
        doc.version = version;
        doc.passages = result.passages.clone();
        (doc, result)
    } else {
        // No plugin available — create an empty document
        tracing::warn!("No format plugin available for {:?}", format);
        let doc = Document::new(uri.clone(), format);
        let result = fmt_plugin::ParseResult {
            passages: Vec::new(),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            is_complete: false,
        };
        (doc, result)
    }
}

// ===========================================================================
// StoryData extraction
// ===========================================================================

/// After parsing a document, check if it contains a `StoryData` passage.
/// If so, parse its JSON body and set `workspace.metadata`.
fn extract_and_set_metadata(workspace: &mut Workspace, doc: &Document, text: &str) {
    if let Some(story_data) = doc.story_data() {
        // Extract the body text of the StoryData passage.
        // The passage span covers the entire passage (header + body).
        // We need to find the body portion after the header line.
        let body_text = extract_passage_body(text, story_data.span.start);

        if let Some(metadata) = parse_story_data_json(&body_text) {
            tracing::info!(
                "Found StoryData: format={:?}, start={}",
                metadata.format,
                metadata.start_passage
            );
            workspace.metadata = Some(metadata);
        }
    }
}

/// Extract the body text of a passage given the byte offset where the
/// passage starts (the `::` header line). The body starts after the first
/// newline following the header.
fn extract_passage_body(full_text: &str, passage_start: usize) -> String {
    let remainder = if passage_start < full_text.len() {
        &full_text[passage_start..]
    } else {
        return String::new();
    };

    // Skip the header line (everything up to and including the first newline)
    if let Some(newline_pos) = remainder.find('\n') {
        remainder[newline_pos + 1..].to_string()
    } else {
        // No body
        String::new()
    }
}

/// Parse the JSON body of a StoryData passage.
///
/// The StoryData body in Twee 3 looks like:
/// ```json
/// {
///   "ifid": "A1B2C3D4-E5F6-7890-1234-567890ABCDEF",
///   "format": "SugarCube",
///   "format-version": "2.36.1",
///   "start": "Prologue"
/// }
/// ```
fn parse_story_data_json(body: &str) -> Option<StoryMetadata> {
    // Find the first `{` in the body — skip any leading whitespace or tags
    let json_start = body.find('{')?;
    let json_text = &body[json_start..];

    let value: serde_json::Value = serde_json::from_str(json_text).ok()?;

    let format_str = value
        .get("format")
        .and_then(|v| v.as_str())
        .unwrap_or("SugarCube");
    let format = format_str
        .parse::<StoryFormat>()
        .unwrap_or_else(|_| StoryFormat::default_format());

    let format_version = value
        .get("format-version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let start_passage = value
        .get("start")
        .and_then(|v| v.as_str())
        .unwrap_or("Start")
        .to_string();

    let ifid = value
        .get("ifid")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(StoryMetadata {
        format,
        format_version,
        start_passage,
        ifid,
    })
}

// ===========================================================================
// Workspace indexing
// ===========================================================================

/// Scan the workspace root for all `.tw` / `.twee` files, parse them with
/// the format plugin, insert into the workspace, build the graph, and run
/// analysis.
async fn index_workspace(
    inner: &tokio::sync::RwLock<crate::state::ServerStateInner>,
    client: &tower_lsp::Client,
) -> Result<(), String> {
    let root_uri = {
        let inner = inner.read().await;
        inner.workspace.root_uri.clone()
    };

    let root_path = root_uri
        .to_file_path()
        .map_err(|_| "Workspace root is not a file:// URI".to_string())?;

    // Collect all .tw/.twee files using walkdir
    let twee_files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(&root_path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            let ext = entry.path().extension().and_then(|e| e.to_str());
            ext == Some("tw") || ext == Some("twee")
        })
        .map(|entry| entry.into_path())
        .collect();

    let total_files = twee_files.len() as u32;
    if total_files == 0 {
        return Ok(());
    }

    client
        .log_message(
            MessageType::INFO,
            format!("Indexing {} Twee files…", total_files),
        )
        .await;

    // Send initial progress notification
    send_index_progress(client, total_files, 0).await;

    let mut parsed_count: u32 = 0;

    for file_path in &twee_files {
        let text = tokio::fs::read_to_string(file_path)
            .await
            .map_err(|e| format!("Failed to read {}: {}", file_path.display(), e))?;

        let uri = Url::from_file_path(file_path)
            .map_err(|_| format!("Invalid file path: {}", file_path.display()))?;

        let mut inner = inner.write().await;
        let format = inner.workspace.resolve_format();

        inner.open_documents.insert(uri.clone(), text.clone());

        let (doc, parse_result) = parse_with_format_plugin(
            &inner.format_registry,
            &uri,
            &text,
            format,
            0, // version 0 for indexed files
        );

        // Store format diagnostics
        inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);

        // Check for StoryData
        extract_and_set_metadata(&mut inner.workspace, &doc, &text);

        inner.workspace.insert_document(doc);
        drop(inner);

        parsed_count += 1;

        // Send progress every 10 files or on the last file
        if parsed_count % 10 == 0 || parsed_count == total_files {
            send_index_progress(client, total_files, parsed_count).await;
        }
    }

    // After all files are loaded, rebuild the graph and run analysis
    let mut inner = inner.write().await;
    rebuild_graph(&mut inner.workspace);
    inner.workspace.mark_indexed();

    let diagnostics = AnalysisEngine::analyze(&inner.workspace);
    let open_docs = inner.open_documents.clone();
    let fmt_diags = inner.format_diagnostics.clone();
    let config = inner.workspace.config.clone();
    drop(inner);

    publish_all_diagnostics(client, &diagnostics, &fmt_diags, &open_docs, &config).await;

    Ok(())
}

/// Send a `knot/indexProgress` notification to the client.
async fn send_index_progress(client: &tower_lsp::Client, total_files: u32, parsed_files: u32) {
    let progress = KnotIndexProgress {
        total_files,
        parsed_files,
    };
    client
        .send_notification::<KnotIndexProgressNotification>(progress)
        .await;
}

// ===========================================================================
// Graph rebuild
// ===========================================================================

/// Rebuild the passage graph from all workspace documents.
fn rebuild_graph(workspace: &mut Workspace) {
    // Collect passage info first (avoid borrow issues)
    let info: Vec<(String, String, bool, bool, Vec<(Option<String>, String)>)> = workspace
        .documents()
        .flat_map(|doc| {
            doc.passages.iter().map(|p| {
                let edges: Vec<(Option<String>, String)> = p
                    .links
                    .iter()
                    .map(|l| (l.display_text.clone(), l.target.clone()))
                    .collect();
                (
                    p.name.clone(),
                    doc.uri.to_string(),
                    p.is_special,
                    p.is_metadata(),
                    edges,
                )
            })
        })
        .collect();

    workspace.graph = knot_core::PassageGraph::new();

    for (name, file_uri, is_special, is_metadata, _edges) in &info {
        let node = PassageNode {
            name: name.clone(),
            file_uri: file_uri.clone(),
            is_special: *is_special,
            is_metadata: *is_metadata,
        };
        workspace.graph.add_passage(node);
    }

    // Add edges after all nodes exist so broken-link detection works.
    for (source, _, _, _, edges) in &info {
        for (display_text, target) in edges {
            let target_exists = workspace.graph.contains_passage(target);
            let edge = PassageEdge {
                display_text: display_text.clone(),
                is_broken: !target_exists,
            };
            workspace.graph.add_edge(source, target, edge);
        }
    }
}

// ===========================================================================
// Diagnostics
// ===========================================================================

/// Count the number of incoming links to a passage from other passages.
fn count_incoming_links(workspace: &Workspace, passage_name: &str) -> usize {
    let mut count = 0;
    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.links.iter().any(|l| l.target == passage_name) {
                count += 1;
            }
        }
    }
    count
}

/// Publish diagnostics to **all** affected files. This combines:
/// - Graph analysis diagnostics (broken links, unreachable, loops, etc.)
/// - Format plugin diagnostics (syntax errors, parsing warnings, etc.)
///
/// Groups results by `file_uri` and publishes each group separately.
async fn publish_all_diagnostics(
    client: &tower_lsp::Client,
    graph_diagnostics: &[knot_core::graph::GraphDiagnostic],
    format_diagnostics: &std::collections::HashMap<Url, Vec<fmt_plugin::FormatDiagnostic>>,
    open_documents: &std::collections::HashMap<Url, String>,
    config: &knot_core::workspace::KnotConfig,
) {
    use std::collections::HashMap;

    // Group graph diagnostics by file URI
    let mut by_file: HashMap<String, Vec<&knot_core::graph::GraphDiagnostic>> = HashMap::new();
    for gd in graph_diagnostics {
        by_file
            .entry(gd.file_uri.clone())
            .or_default()
            .push(gd);
    }

    // Collect all files that should have diagnostics published
    let all_uris: std::collections::HashSet<Url> = open_documents
        .keys()
        .chain(format_diagnostics.keys())
        .cloned()
        .collect();

    for uri in &all_uris {
        let uri_str = uri.to_string();
        let text = open_documents.get(uri).map(|s| s.as_str()).unwrap_or("");

        let mut lsp_diagnostics: Vec<Diagnostic> = Vec::new();

        // Add graph diagnostics for this file
        if let Some(diags) = by_file.get(&uri_str) {
            for gd in diags {
                let default_severity = match gd.kind {
                    DiagnosticKind::BrokenLink => DiagnosticSeverity::WARNING,
                    DiagnosticKind::UnreachablePassage => DiagnosticSeverity::HINT,
                    DiagnosticKind::InfiniteLoop => DiagnosticSeverity::WARNING,
                    DiagnosticKind::UninitializedVariable => DiagnosticSeverity::WARNING,
                    DiagnosticKind::UnusedVariable => DiagnosticSeverity::HINT,
                    DiagnosticKind::RedundantWrite => DiagnosticSeverity::HINT,
                    DiagnosticKind::DuplicateStoryData => DiagnosticSeverity::ERROR,
                    DiagnosticKind::MissingStoryData => DiagnosticSeverity::WARNING,
                    DiagnosticKind::MissingStartPassage => DiagnosticSeverity::ERROR,
                    DiagnosticKind::UnsupportedFormat => DiagnosticSeverity::ERROR,
                    DiagnosticKind::DuplicatePassageName => DiagnosticSeverity::ERROR,
                    DiagnosticKind::EmptyPassage => DiagnosticSeverity::HINT,
                    DiagnosticKind::DeadEndPassage => DiagnosticSeverity::INFORMATION,
                    DiagnosticKind::InvalidPassageName => DiagnosticSeverity::WARNING,
                    DiagnosticKind::OrphanedPassage => DiagnosticSeverity::INFORMATION,
                    DiagnosticKind::ComplexPassage => DiagnosticSeverity::HINT,
                    DiagnosticKind::LargePassage => DiagnosticSeverity::HINT,
                    DiagnosticKind::MissingStartLink => DiagnosticSeverity::WARNING,
                };

                // Check config for severity override or suppression
                let diag_key = format!("{:?}", gd.kind);
                let severity = if let Some(custom) = config.diagnostics.get(&diag_key) {
                    match custom {
                        knot_core::workspace::DiagnosticSeverity::Off => continue, // Suppress
                        knot_core::workspace::DiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                        knot_core::workspace::DiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                        knot_core::workspace::DiagnosticSeverity::Info => DiagnosticSeverity::INFORMATION,
                        knot_core::workspace::DiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
                    }
                } else {
                    default_severity
                };

                // For graph diagnostics, find the passage header range
                let range = find_passage_header_range(text, &gd.passage_name);

                // Build related information pointing to source or target passages
                let related_information = build_related_information_for_push(
                    open_documents, &gd.kind, &gd.passage_name, &gd.message,
                );

                lsp_diagnostics.push(Diagnostic {
                    range,
                    severity: Some(severity),
                    code: Some(NumberOrString::String(format!("{:?}", gd.kind))),
                    source: Some("knot".to_string()),
                    message: gd.message.clone(),
                    related_information,
                    ..Default::default()
                });
            }
        }

        // Add format plugin diagnostics for this file
        if let Some(fmt_diags) = format_diagnostics.get(uri) {
            for fd in fmt_diags {
                let range = byte_range_to_lsp_range(text, &fd.range);

                let severity = match fd.severity {
                    fmt_plugin::FormatDiagnosticSeverity::Error => DiagnosticSeverity::ERROR,
                    fmt_plugin::FormatDiagnosticSeverity::Warning => DiagnosticSeverity::WARNING,
                    fmt_plugin::FormatDiagnosticSeverity::Info => DiagnosticSeverity::INFORMATION,
                    fmt_plugin::FormatDiagnosticSeverity::Hint => DiagnosticSeverity::HINT,
                };

                lsp_diagnostics.push(Diagnostic {
                    range,
                    severity: Some(severity),
                    code: Some(NumberOrString::String(format!("format:{}", fd.code))),
                    source: Some("knot".to_string()),
                    message: fd.message.clone(),
                    ..Default::default()
                });
            }
        }

        client
            .publish_diagnostics(uri.clone(), lsp_diagnostics, None)
            .await;
    }
}

// ===========================================================================
// Position / Range helpers
// ===========================================================================

/// Convert a byte offset to an LSP Position (0-based line & character).
fn byte_offset_to_position(text: &str, offset: usize) -> Position {
    let safe_offset = offset.min(text.len());
    let text_before = &text[..safe_offset];
    let line = text_before.lines().count() as u32;
    let line = if text_before.is_empty() || text_before.ends_with('\n') {
        line
    } else {
        line - 1
    };
    let last_newline = text_before.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = (safe_offset - last_newline) as u32;
    Position { line, character }
}

/// Convert a byte range to an LSP Range.
fn byte_range_to_lsp_range(text: &str, range: &std::ops::Range<usize>) -> Range {
    let start = byte_offset_to_position(text, range.start);
    let end = byte_offset_to_position(text, range.end);
    Range { start, end }
}

/// Find the LSP Range for a passage header line.
fn find_passage_header_range(text: &str, passage_name: &str) -> Range {
    for (line_idx, line) in text.lines().enumerate() {
        if line.starts_with("::") {
            let name = parse_passage_name_from_header(&line[2..]);
            if name == passage_name {
                return Range {
                    start: Position {
                        line: line_idx as u32,
                        character: 0,
                    },
                    end: Position {
                        line: line_idx as u32,
                        character: line.len() as u32,
                    },
                };
            }
        }
    }
    Range::default()
}

/// Parse just the passage name from a header (the part after `::`).
fn parse_passage_name_from_header(header: &str) -> String {
    let header = header.trim();
    if let Some(bracket_start) = header.find('[') {
        header[..bracket_start].trim().to_string()
    } else {
        header.to_string()
    }
}

/// Find the passage name at a given LSP position.
fn find_passage_at_position(text: &str, position: Position) -> Option<String> {
    let line_text = text.lines().nth(position.line as usize)?;
    if line_text.starts_with("::") {
        let name = parse_passage_name_from_header(&line_text[2..]);
        Some(name)
    } else {
        None
    }
}

/// Find a link target at a given LSP position.
fn find_link_target_at_position(text: &str, position: Position) -> Option<String> {
    let line_text = text.lines().nth(position.line as usize)?;
    let char_offset = position.character as usize;

    let mut search_from = 0;
    while let Some(rel_start) = line_text[search_from..].find("[[") {
        let abs_start = search_from + rel_start;
        if let Some(rel_end) = line_text[abs_start..].find("]]") {
            let content_start = abs_start + 2;
            let content_end = abs_start + rel_end;

            if char_offset >= content_start && char_offset <= content_end {
                let link_text = &line_text[content_start..content_end];
                // Handle both arrow (->) and pipe (|) link syntax
                let target = if let Some(arrow) = link_text.find("->") {
                    &link_text[arrow + 2..]
                } else if let Some(pipe) = link_text.find('|') {
                    &link_text[pipe + 1..]
                } else {
                    link_text
                };
                let target = target.trim();
                if !target.is_empty() {
                    return Some(target.to_string());
                }
            }
            search_from = abs_start + rel_end + 2;
        } else {
            break;
        }
    }
    None
}

// ===========================================================================
// Semantic token helpers
// ===========================================================================

/// Intermediate token used during semantic-token conversion.
struct SemTok {
    line: u32,
    start_char: u32,
    length: u32,
    token_type: u32,
    token_modifiers: u32,
}

/// Convert format-plugin semantic tokens (byte-offset based) to the
/// intermediate `SemTok` representation (line/character based).
fn convert_semantic_tokens(
    text: &str,
    plugin_tokens: &[fmt_plugin::SemanticToken],
) -> Vec<SemTok> {
    let mut tokens = Vec::new();

    for pt in plugin_tokens {
        let pos = byte_offset_to_position(text, pt.start);
        let token_type = map_token_type(&pt.token_type);
        let modifiers = map_token_modifier(&pt.modifier);

        tokens.push(SemTok {
            line: pos.line,
            start_char: pos.character,
            length: pt.length as u32,
            token_type,
            token_modifiers: modifiers,
        });
    }

    tokens
}

/// Map a `knot_formats::plugin::SemanticTokenType` to the LSP legend index.
fn map_token_type(tt: &fmt_plugin::SemanticTokenType) -> u32 {
    match tt {
        fmt_plugin::SemanticTokenType::PassageHeader => ST_PASSAGE_HEADER,
        fmt_plugin::SemanticTokenType::Link => ST_LINK,
        fmt_plugin::SemanticTokenType::Macro => ST_MACRO,
        fmt_plugin::SemanticTokenType::Variable => ST_VARIABLE,
        fmt_plugin::SemanticTokenType::String => ST_STRING,
        fmt_plugin::SemanticTokenType::Number => ST_NUMBER,
        fmt_plugin::SemanticTokenType::Boolean => ST_BOOLEAN,
        fmt_plugin::SemanticTokenType::Comment => ST_COMMENT,
        fmt_plugin::SemanticTokenType::Tag => ST_TAG,
        fmt_plugin::SemanticTokenType::Keyword => ST_KEYWORD,
    }
}

/// Map an optional `SemanticTokenModifier` to the LSP modifier bitset.
fn map_token_modifier(modifier: &Option<fmt_plugin::SemanticTokenModifier>) -> u32 {
    match modifier {
        Some(fmt_plugin::SemanticTokenModifier::Definition) => SM_DEFINITION,
        Some(fmt_plugin::SemanticTokenModifier::ReadOnly) => SM_READONLY,
        Some(fmt_plugin::SemanticTokenModifier::Deprecated) => SM_DEPRECATED,
        Some(fmt_plugin::SemanticTokenModifier::ControlFlow) => SM_CONTROLFLOW,
        None => 0,
    }
}

/// Delta-encode semantic tokens into the LSP wire format.
fn encode_semantic_tokens(tokens: &[SemTok]) -> Vec<lsp_types::SemanticToken> {
    let mut sorted: Vec<&SemTok> = tokens.iter().collect();
    sorted.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.start_char.cmp(&b.start_char)));

    let mut data = Vec::with_capacity(sorted.len());
    let mut prev_line = 0u32;
    let mut prev_start = 0u32;

    for tok in sorted {
        let delta_line = tok.line - prev_line;
        let delta_start = if delta_line == 0 {
            tok.start_char - prev_start
        } else {
            tok.start_char
        };

        data.push(lsp_types::SemanticToken {
            delta_line,
            delta_start,
            length: tok.length,
            token_type: tok.token_type,
            token_modifiers_bitset: tok.token_modifiers,
        });

        prev_line = tok.line;
        prev_start = tok.start_char;
    }

    data
}

// ===========================================================================
// Compiler helpers
// ===========================================================================

/// Search for the Tweego compiler on the system PATH.
fn which_compiler() -> Option<std::path::PathBuf> {
    // Try common compiler names
    for name in &["tweego", "tweego.exe"] {
        if let Ok(output) = std::process::Command::new("which")
            .arg(name)
            .output()
        {
            if output.status.success() {
                let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let path = std::path::PathBuf::from(&path_str);
                if path.exists() {
                    return Some(path);
                }
            }
        }
    }

    // Try direct execution as fallback
    for name in &["tweego", "tweego.exe"] {
        if std::process::Command::new(name)
            .arg("--version")
            .output()
            .is_ok()
        {
            return Some(std::path::PathBuf::from(name));
        }
    }

    None
}

/// Detect the version string of a compiler by running `--version`.
async fn detect_compiler_version(path: &std::path::Path) -> Option<String> {
    let output = tokio::process::Command::new(path)
        .arg("--version")
        .output()
        .await
        .ok()?;

    if output.status.success() {
        let version = String::from_utf8_lossy(&output.stdout);
        // Take the first line of output as the version string
        Some(version.lines().next().unwrap_or("").to_string())
    } else {
        None
    }
}

/// Compute the maximum depth from the start passage using BFS.
fn compute_max_depth(workspace: &Workspace, start_passage: &str) -> u32 {
    let mut depths: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();

    if workspace.graph.contains_passage(start_passage) {
        depths.insert(start_passage.to_string(), 0);
        queue.push_back(start_passage.to_string());
    }

    while let Some(name) = queue.pop_front() {
        let current_depth = *depths.get(&name).unwrap_or(&0);
        for neighbor in workspace.graph.outgoing_neighbors(&name) {
            if !depths.contains_key(&neighbor) {
                let new_depth = current_depth + 1;
                depths.insert(neighbor.clone(), new_depth);
                queue.push_back(neighbor);
            }
        }
    }

    depths.values().copied().max().unwrap_or(0)
}

/// Compute the number of weakly connected components in the passage graph.
fn compute_connected_components(workspace: &Workspace) -> u32 {
    let passage_names: Vec<String> = workspace.graph.passage_names();
    if passage_names.is_empty() {
        return 0;
    }

    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut component_count: u32 = 0;

    for name in &passage_names {
        if visited.contains(name) {
            continue;
        }

        // BFS considering both directions (weakly connected)
        component_count += 1;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(name.clone());
        visited.insert(name.clone());

        while let Some(current) = queue.pop_front() {
            for neighbor in workspace.graph.outgoing_neighbors(&current) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
            for neighbor in workspace.graph.incoming_neighbors(&current) {
                if visited.insert(neighbor.clone()) {
                    queue.push_back(neighbor);
                }
            }
        }
    }

    component_count
}

/// Compute a simplified average clustering coefficient.
///
/// For each passage, count how many of its outgoing neighbors also link
/// to each other (forming triangles), divided by the maximum possible
/// number of such connections. Returns the average across all passages
/// with at least 2 outgoing links.
fn compute_avg_clustering(workspace: &Workspace) -> f64 {
    let passage_names: Vec<String> = workspace.graph.passage_names();
    let mut coefficients: Vec<f64> = Vec::new();

    for name in &passage_names {
        let out_neighbors: Vec<String> = workspace.graph.outgoing_neighbors(name);
        let k = out_neighbors.len();

        if k < 2 {
            continue;
        }

        let neighbor_set: std::collections::HashSet<String> =
            out_neighbors.iter().cloned().collect();

        let mut triangle_count: u32 = 0;
        for neighbor in &out_neighbors {
            let their_neighbors = workspace.graph.outgoing_neighbors(neighbor);
            for their_target in &their_neighbors {
                if neighbor_set.contains(their_target) {
                    triangle_count += 1;
                }
            }
        }

        let max_possible = (k * (k - 1)) as f64;
        let local_coeff = triangle_count as f64 / max_possible;
        coefficients.push(local_coeff);
    }

    if coefficients.is_empty() {
        0.0
    } else {
        coefficients.iter().sum::<f64>() / coefficients.len() as f64
    }
}

// ===========================================================================
// SugarCube macro signature table
// ===========================================================================

/// A SugarCube macro signature entry.
struct MacroSignature {
    name: &'static str,
    signature: &'static str,
    description: &'static str,
    has_params: bool,
}

impl MacroSignature {
    /// Return the snippet portion after the macro name (for insertion).
    fn insertSnippet(&self) -> &'static str {
        if self.has_params {
            " ${1:args}"
        } else {
            ""
        }
    }

    /// Return parameter names for signature help.
    fn param_names(&self) -> Vec<String> {
        if self.signature.is_empty() {
            return vec![];
        }
        self.signature
            .split(',')
            .map(|s| s.trim().to_string())
            .collect()
    }
}

/// Built-in SugarCube macro signatures.
fn sugarcube_macro_signatures() -> Vec<MacroSignature> {
    vec![
        MacroSignature { name: "set", signature: "$var to expr", description: "Set a variable to a value.\n\nExample: `<<set $gold to 100>>`", has_params: true },
        MacroSignature { name: "if", signature: "condition", description: "Conditional block — executes content if condition is true.\n\nExample: `<<if $gold > 50>>`", has_params: true },
        MacroSignature { name: "elseif", signature: "condition", description: "Else-if clause for conditional blocks.", has_params: true },
        MacroSignature { name: "else", signature: "", description: "Else clause for conditional blocks.", has_params: false },
        MacroSignature { name: "for", signature: "$var, $var2, ... to expr", description: "Iterate over a collection or range.\n\nExample: `<<for _i to 0; _i < 5; _i++>>`", has_params: true },
        MacroSignature { name: "switch", signature: "expr", description: "Switch statement for multi-way branching.", has_params: true },
        MacroSignature { name: "case", signature: "value", description: "Case clause within a switch block.", has_params: true },
        MacroSignature { name: "include", signature: "passageName", description: "Include the content of another passage inline.\n\nExample: `<<include \"Sidebar\">>`", has_params: true },
        MacroSignature { name: "print", signature: "expr", description: "Print the result of an expression.\n\nExample: `<<print $gold>>`", has_params: true },
        MacroSignature { name: "nobr", signature: "", description: "Suppress automatic line break handling within the block.", has_params: false },
        MacroSignature { name: "script", signature: "", description: "Include raw JavaScript code.", has_params: false },
        MacroSignature { name: "run", signature: "expr", description: "Run a JavaScript expression silently (no output).\n\nExample: `<<run state.active.passage = 'Start'>>`", has_params: true },
        MacroSignature { name: "capture", signature: "$var", description: "Capture rendered content into a variable.", has_params: true },
        MacroSignature { name: "append", signature: "selector", description: "Append content to a DOM element matching the selector.", has_params: true },
        MacroSignature { name: "prepend", signature: "selector", description: "Prepend content to a DOM element matching the selector.", has_params: true },
        MacroSignature { name: "replace", signature: "selector", description: "Replace the content of a DOM element matching the selector.", has_params: true },
        MacroSignature { name: "remove", signature: "selector", description: "Remove a DOM element matching the selector.", has_params: true },
        MacroSignature { name: "button", signature: "label, passageName", description: "Create a button that navigates to a passage.", has_params: true },
        MacroSignature { name: "link", signature: "label, passageName", description: "Create a passage link with optional display text.", has_params: true },
        MacroSignature { name: "actions", signature: "", description: "Container for `<<choice>>` macros.", has_params: false },
        MacroSignature { name: "choice", signature: "label, passageName", description: "Create a one-time choice within an `<<actions>>` block.", has_params: true },
        MacroSignature { name: "goto", signature: "passageName", description: "Immediately navigate to another passage.\n\nExample: `<<goto \"EndGame\">>`", has_params: true },
        MacroSignature { name: "back", signature: "", description: "Navigate to the previous passage in history.", has_params: false },
        MacroSignature { name: "return", signature: "", description: "Return from an `<<include>>` or `<<widget>>`.", has_params: false },
        MacroSignature { name: "widget", signature: "widgetName", description: "Define a reusable widget macro.\n\nExample: `<<widget \"hello\">>Hello!<</widget>>`", has_params: true },
        MacroSignature { name: "type", signature: "text, speed", description: "Typewriter effect for text content.", has_params: true },
        MacroSignature { name: "timed", signature: "delay", description: "Execute content after a delay (in milliseconds or CSS time).\n\nExample: `<<timed 2s>>`", has_params: true },
        MacroSignature { name: "next", signature: "delay", description: "Chain additional timed content after a `<<timed>>` block.", has_params: true },
        MacroSignature { name: "visit", signature: "passageName", description: "Check if a passage has been visited and how many times.", has_params: true },
    ]
}

// ===========================================================================
// Formatting helpers
// ===========================================================================

/// Format a Twee document: normalize headers, trim trailing whitespace,
/// ensure blank lines between passages.
fn format_twee_text(text: &str) -> Vec<TextEdit> {
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut edits = Vec::new();
    let mut in_passage = false;

    for (i, line) in lines.iter().enumerate() {
        // Trim trailing whitespace
        let trimmed_end = line.trim_end();
        if trimmed_end.len() != line.len() {
            edits.push(TextEdit {
                range: Range {
                    start: Position { line: i as u32, character: trimmed_end.len() as u32 },
                    end: Position { line: i as u32, character: line.len() as u32 },
                },
                new_text: String::new(),
            });
        }

        // Normalize passage header spacing: ensure exactly one space after "::"
        if line.starts_with("::") {
            in_passage = true;
            let rest = &line[2..];
            if rest.starts_with(|c: char| c != ' ' && c != '[' && c != '\t') && !rest.is_empty() {
                // Missing space after "::", add one
                edits.push(TextEdit {
                    range: Range {
                        start: Position { line: i as u32, character: 2 },
                        end: Position { line: i as u32, character: 2 },
                    },
                    new_text: " ".to_string(),
                });
            }
        }
    }

    // Ensure blank lines between passages — done as a full replacement if needed
    let mut formatted_lines: Vec<String> = Vec::new();
    let mut prev_was_blank = true; // start with blank to avoid blank line at top

    for line in &lines {
        if line.starts_with("::") {
            if !prev_was_blank && !formatted_lines.is_empty() {
                formatted_lines.push(String::new());
            }
            formatted_lines.push(line.trim_end().to_string());
            prev_was_blank = false;
        } else {
            let trimmed = line.trim_end().to_string();
            prev_was_blank = trimmed.is_empty();
            formatted_lines.push(trimmed);
        }
    }

    let formatted_text = formatted_lines.join("\n");
    let original_text = text.to_string();

    if formatted_text != original_text {
        // Return a single edit replacing the entire document
        let line_count = lines.len() as u32;
        let last_line_len = lines.last().map(|l| l.len()).unwrap_or(0) as u32;
        vec![TextEdit {
            range: Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: line_count.saturating_sub(1), character: last_line_len },
            },
            new_text: formatted_text,
        }]
    } else {
        edits
    }
}

// ===========================================================================
// Code Action helpers
// ===========================================================================

/// Extract a quoted name from a diagnostic message, e.g. "Broken link to 'Foo'"
fn extract_quoted_name(message: &str) -> Option<String> {
    if let Some(start) = message.find('\'') {
        let rest = &message[start + 1..];
        if let Some(end) = rest.find('\'') {
            return Some(rest[..end].to_string());
        }
    }
    // Also try double quotes
    if let Some(start) = message.find('"') {
        let rest = &message[start + 1..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

/// Extract a passage name from a diagnostic message.
fn extract_passage_from_diag(message: &str) -> Option<String> {
    extract_quoted_name(message).or_else(|| {
        // Try to extract after "passage" keyword
        let lower = message.to_lowercase();
        if let Some(idx) = lower.find("passage ") {
            let rest = &message[idx + 8..];
            let name = rest.split_whitespace().next().unwrap_or("").trim_end_matches(':').to_string();
            if !name.is_empty() { Some(name) } else { None }
        } else {
            None
        }
    })
}

/// Extract a variable name from a diagnostic message.
fn extract_variable_name(message: &str) -> Option<String> {
    // Look for $varname pattern
    for word in message.split_whitespace() {
        if word.starts_with('$') {
            return Some(word.trim_end_matches(':').trim_end_matches(',').to_string());
        }
    }
    None
}

/// Create a WorkspaceEdit that creates a new passage at the end of the file
/// where the broken link occurs.
fn create_passage_edit(
    inner: &crate::state::ServerStateInner,
    name: &str,
) -> WorkspaceEdit {
    // Find any open document to add the passage to (prefer the one with StoryData)
    let target_uri = inner.workspace.find_passage_file_uri("StoryData")
        .or_else(|| {
            inner.open_documents.keys().next().cloned()
        });

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some(uri) = target_uri {
        if let Some(text) = inner.open_documents.get(&uri) {
            let line_count = text.lines().count() as u32;
            changes.insert(uri, vec![TextEdit {
                range: Range {
                    start: Position { line: line_count, character: 0 },
                    end: Position { line: line_count, character: 0 },
                },
                new_text: format!("\n:: {}\n", name),
            }]);
        }
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Find the nearest reachable passage to a given unreachable one.
fn find_nearest_reachable_passage(workspace: &Workspace, name: &str) -> Option<String> {
    let start_passage = workspace.metadata.as_ref()
        .map(|m| m.start_passage.as_str())
        .unwrap_or("Start");

    let unreachable = workspace.graph.detect_unreachable(start_passage);
    let unreachable_set: std::collections::HashSet<String> = unreachable.iter()
        .map(|d| d.passage_name.clone())
        .collect();

    // Look for passages that link to passages that are near the unreachable one
    // Try finding a passage in the same file first
    if let Some((doc, _)) = workspace.find_passage(name) {
        for passage in &doc.passages {
            if !unreachable_set.contains(&passage.name) && passage.name != name {
                return Some(passage.name.clone());
            }
        }
    }

    // Fall back to the start passage
    if workspace.find_passage(start_passage).is_some() {
        Some(start_passage.to_string())
    } else {
        None
    }
}

/// Create a WorkspaceEdit that adds a link from one passage to another.
fn add_link_edit(
    inner: &crate::state::ServerStateInner,
    from_passage: &str,
    to_passage: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some((doc, _)) = inner.workspace.find_passage(from_passage) {
        if let Some(text) = inner.open_documents.get(&doc.uri) {
            // Find the passage header line and add a link at the end of its body
            let mut header_line: Option<u32> = None;
            let mut end_line: u32 = 0;
            for (i, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let pname = parse_passage_name_from_header(&line[2..]);
                    if pname == from_passage {
                        header_line = Some(i as u32);
                    } else if header_line.is_some() {
                        end_line = i as u32;
                        break;
                    }
                }
                if header_line.is_some() {
                    end_line = i as u32 + 1;
                }
            }

            if let Some(hl) = header_line {
                let insert_line = if end_line > hl { end_line } else { hl + 1 };
                changes.insert(doc.uri.clone(), vec![TextEdit {
                    range: Range {
                        start: Position { line: insert_line, character: 0 },
                        end: Position { line: insert_line, character: 0 },
                    },
                    new_text: format!("[[{}]]\n", to_passage),
                }]);
            }
        }
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create a WorkspaceEdit that adds content template to an empty passage.
fn add_content_template_edit(
    inner: &crate::state::ServerStateInner,
    name: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    if let Some((doc, _)) = inner.workspace.find_passage(name) {
        if let Some(text) = inner.open_documents.get(&doc.uri) {
            for (i, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let pname = parse_passage_name_from_header(&line[2..]);
                    if pname == name {
                        // Insert template content after the header line
                        let template = format!("{}\n", name);
                        changes.insert(doc.uri.clone(), vec![TextEdit {
                            range: Range {
                                start: Position { line: (i as u32) + 1, character: 0 },
                                end: Position { line: (i as u32) + 1, character: 0 },
                            },
                            new_text: template,
                        }]);
                        break;
                    }
                }
            }
        }
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

/// Create a WorkspaceEdit that initializes a variable in StoryInit.
fn initialize_var_in_story_init_edit(
    inner: &crate::state::ServerStateInner,
    var_name: &str,
) -> WorkspaceEdit {
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    // Find StoryInit passage
    if let Some((doc, _)) = inner.workspace.find_passage("StoryInit") {
        if let Some(text) = inner.open_documents.get(&doc.uri) {
            // Find the last line of StoryInit
            let mut init_start: Option<u32> = None;
            let mut init_end: u32 = 0;
            for (i, line) in text.lines().enumerate() {
                if line.starts_with("::") {
                    let pname = parse_passage_name_from_header(&line[2..]);
                    if pname == "StoryInit" {
                        init_start = Some(i as u32);
                    } else if init_start.is_some() {
                        init_end = i as u32;
                        break;
                    }
                }
                if init_start.is_some() {
                    init_end = i as u32 + 1;
                }
            }

            let insert_line = if init_end > init_start.unwrap_or(0) { init_end } else { init_start.unwrap_or(0) + 1 };
            changes.insert(doc.uri.clone(), vec![TextEdit {
                range: Range {
                    start: Position { line: insert_line, character: 0 },
                    end: Position { line: insert_line, character: 0 },
                },
                new_text: format!("<<set {} to 0>>\n", var_name),
            }]);
        }
    } else {
        // No StoryInit — create one
        if let Some(uri) = inner.open_documents.keys().next() {
            if let Some(text) = inner.open_documents.get(uri) {
                let line_count = text.lines().count() as u32;
                changes.insert(uri.clone(), vec![TextEdit {
                    range: Range {
                        start: Position { line: line_count, character: 0 },
                        end: Position { line: line_count, character: 0 },
                    },
                    new_text: format!("\n:: StoryInit\n<<set {} to 0>>\n", var_name),
                }]);
            }
        }
    }

    WorkspaceEdit {
        changes: Some(changes),
        document_changes: None,
        change_annotations: None,
    }
}

// ===========================================================================
// Related Information helpers
// ===========================================================================

/// Build related information for push diagnostics (publish_all_diagnostics).
fn build_related_information_for_push(
    open_documents: &HashMap<Url, String>,
    kind: &DiagnosticKind,
    passage_name: &str,
    _message: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    match kind {
        DiagnosticKind::BrokenLink => {
            // Point to the passage that contains the broken link
            find_link_locations(open_documents, passage_name)
        }
        DiagnosticKind::UnreachablePassage => {
            // Point to passages that should link to this one
            find_definition_location(open_documents, passage_name)
        }
        DiagnosticKind::DuplicatePassageName => {
            // Point to all definitions of this passage name
            find_all_definition_locations(open_documents, passage_name)
        }
        DiagnosticKind::UninitializedVariable => {
            // Point to where the variable is first read
            find_variable_read_locations(open_documents, passage_name)
        }
        _ => None,
    }
}

/// Build related information for pull diagnostics (diagnostic method).
fn build_related_information(
    inner: &crate::state::ServerStateInner,
    kind: &DiagnosticKind,
    passage_name: &str,
    _message: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    match kind {
        DiagnosticKind::BrokenLink => {
            find_link_locations(&inner.open_documents, passage_name)
        }
        DiagnosticKind::UnreachablePassage => {
            find_definition_location(&inner.open_documents, passage_name)
        }
        DiagnosticKind::DuplicatePassageName => {
            find_all_definition_locations(&inner.open_documents, passage_name)
        }
        DiagnosticKind::UninitializedVariable => {
            find_variable_read_locations(&inner.open_documents, passage_name)
        }
        _ => None,
    }
}

/// Find locations of links to a given passage name (for broken link related info).
fn find_link_locations(
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let mut related = Vec::new();
    for (doc_uri, text) in open_documents {
        for (line_idx, line) in text.lines().enumerate() {
            let mut search_from = 0;
            while let Some(rel_start) = line[search_from..].find("[[") {
                let abs_start = search_from + rel_start;
                if let Some(rel_end) = line[abs_start..].find("]]") {
                    let content_start = abs_start + 2;
                    let content_end = abs_start + rel_end;
                    let link_text = &line[content_start..content_end];
                    let link_target = if let Some(arrow) = link_text.find("->") {
                        &link_text[arrow + 2..]
                    } else if let Some(pipe) = link_text.find('|') {
                        &link_text[pipe + 1..]
                    } else {
                        link_text
                    };
                    if link_target.trim() == passage_name {
                        related.push(DiagnosticRelatedInformation {
                            location: Location {
                                uri: doc_uri.clone(),
                                range: Range {
                                    start: Position { line: line_idx as u32, character: content_start as u32 },
                                    end: Position { line: line_idx as u32, character: content_end as u32 },
                                },
                            },
                            message: format!("Link to '{}'", passage_name),
                        });
                    }
                    search_from = abs_start + rel_end + 2;
                } else {
                    break;
                }
            }
        }
    }
    if related.is_empty() { None } else { Some(related) }
}

/// Find the definition location of a passage (for unreachable passage related info).
fn find_definition_location(
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    for (doc_uri, text) in open_documents {
        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let name = parse_passage_name_from_header(&line[2..]);
                if name == passage_name {
                    return Some(vec![DiagnosticRelatedInformation {
                        location: Location {
                            uri: doc_uri.clone(),
                            range: Range {
                                start: Position { line: line_idx as u32, character: 0 },
                                end: Position { line: line_idx as u32, character: line.len() as u32 },
                            },
                        },
                        message: format!("Definition of '{}'", passage_name),
                    }]);
                }
            }
        }
    }
    None
}

/// Find all definition locations of a passage name (for duplicate passage diagnostics).
fn find_all_definition_locations(
    open_documents: &HashMap<Url, String>,
    passage_name: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    let mut related = Vec::new();
    for (doc_uri, text) in open_documents {
        for (line_idx, line) in text.lines().enumerate() {
            if line.starts_with("::") {
                let name = parse_passage_name_from_header(&line[2..]);
                if name == passage_name {
                    related.push(DiagnosticRelatedInformation {
                        location: Location {
                            uri: doc_uri.clone(),
                            range: Range {
                                start: Position { line: line_idx as u32, character: 0 },
                                end: Position { line: line_idx as u32, character: line.len() as u32 },
                            },
                        },
                        message: format!("Definition of '{}'", passage_name),
                    });
                }
            }
        }
    }
    if related.is_empty() { None } else { Some(related) }
}

/// Find locations where a variable is read (for uninitialized variable diagnostics).
fn find_variable_read_locations(
    open_documents: &HashMap<Url, String>,
    _passage_name: &str,
) -> Option<Vec<DiagnosticRelatedInformation>> {
    // For now, we don't trace specific variable read locations from the message
    // This could be enhanced by parsing the message for variable names
    None
}

// ===========================================================================
// Other helper functions
// ===========================================================================

/// Find passages that link TO a given passage name.
fn find_passages_linking_to(workspace: &Workspace, passage_name: &str) -> Vec<String> {
    let mut result = Vec::new();
    for doc in workspace.documents() {
        for passage in &doc.passages {
            if passage.links.iter().any(|l| l.target == passage_name) {
                result.push(passage.name.clone());
            }
        }
    }
    result.sort();
    result.dedup();
    result
}

/// Find all [[target]] link ranges for a specific target in the text.
fn find_link_ranges_for_target(text: &str, target: &str) -> Vec<Range> {
    let mut ranges = Vec::new();
    for (line_idx, line) in text.lines().enumerate() {
        let mut search_from = 0;
        while let Some(rel_start) = line[search_from..].find("[[") {
            let abs_start = search_from + rel_start;
            if let Some(rel_end) = line[abs_start..].find("]]") {
                let content_start = abs_start + 2;
                let content_end = abs_start + rel_end;
                let link_text = &line[content_start..content_end];
                let link_target = if let Some(arrow) = link_text.find("->") {
                    &link_text[arrow + 2..]
                } else if let Some(pipe) = link_text.find('|') {
                    &link_text[pipe + 1..]
                } else {
                    link_text
                };
                if link_target.trim() == target {
                    ranges.push(Range {
                        start: Position { line: line_idx as u32, character: content_start as u32 },
                        end: Position { line: line_idx as u32, character: content_end as u32 },
                    });
                }
                search_from = abs_start + rel_end + 2;
            } else {
                break;
            }
        }
    }
    ranges
}
