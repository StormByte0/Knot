//! Document synchronization handlers: did_open, did_change, did_close,
//! did_save, did_change_configuration, did_change_watched_files.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_core::editing::graph_surgery;
use knot_core::passage::Passage;
use knot_core::AnalysisEngine;
use lsp_types::*;
pub(crate) async fn did_open(state: &ServerState, params: DidOpenTextDocumentParams) {
    let uri = params.text_document.uri;
    let text = params.text_document.text;
    let version = params.text_document.version;

    tracing::info!("did_open: {}", uri);

    let mut inner = state.inner.write().await;
    inner.open_documents.insert(uri.clone(), text.clone());

    let format = inner.workspace.resolve_format();
    let (doc, parse_result) =
        helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format, version);

    // Store format diagnostics for this document
    inner.format_diagnostics.insert(
        uri.clone(),
        parse_result.diagnostics.clone(),
    );

    // Check for StoryData in the newly opened document
    helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);

    inner.workspace.insert_document(doc);
    helpers::rebuild_graph(&mut inner.workspace);

    let diagnostics = AnalysisEngine::analyze(&inner.workspace);
    let open_docs = inner.open_documents.clone();
    let fmt_diags = inner.format_diagnostics.clone();
    let config = inner.workspace.config.clone();
    drop(inner);

    helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
}

pub(crate) async fn did_change(state: &ServerState, params: DidChangeTextDocumentParams) {
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

    let mut inner = state.inner.write().await;

    // Debounce — record the edit time
    inner.debounce.record_edit();

    // Always update the text cache immediately so go-to-definition etc.
    // see the latest content, even if we skip heavy analysis this round.
    inner.open_documents.insert(uri.clone(), text.clone());

    // If we're still inside the debounce window, skip the expensive
    // parse + graph surgery + analysis pass. The next did_change that
    // arrives after the debounce window will carry the full text and
    // trigger a complete re-analysis. This trades a small delay in
    // diagnostic updates for significantly less CPU usage during rapid
    // typing bursts.
    if inner.debounce.is_pending() {
        tracing::debug!("did_change: debounced — skipping full analysis for {}", uri);
        inner.debounce.mark_skipped();
        drop(inner);
        return;
    }

    // If a previous edit was skipped and the debounce window has now expired,
    // we must still run full analysis to catch up.
    if inner.debounce.needs_flush() {
        inner.debounce.clear_skipped();
        // Fall through to the full analysis below
    }

    // Parse with format plugin
    let format = inner.workspace.resolve_format();
    let (doc, parse_result) =
        helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format, version);

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
    helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);

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

    helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
}

pub(crate) async fn did_close(state: &ServerState, params: DidCloseTextDocumentParams) {
    let uri = params.text_document.uri;
    tracing::info!("did_close: {}", uri);

    let mut inner = state.inner.write().await;
    inner.open_documents.remove(&uri);
    inner.format_diagnostics.remove(&uri);
    drop(inner);

    // Clear diagnostics for the closed file.
    state.client
        .publish_diagnostics(uri, Vec::new(), None)
        .await;
}

pub(crate) async fn did_save(_state: &ServerState, params: DidSaveTextDocumentParams) {
    tracing::info!("did_save: {}", params.text_document.uri);
}

pub(crate) async fn did_change_configuration(state: &ServerState, _params: DidChangeConfigurationParams) {
    tracing::info!("did_change_configuration");

    // Re-read .vscode/knot.json in case it was changed externally
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

    let config_values = state
        .client
        .configuration(config_items)
        .await
        .unwrap_or_default();

    // Apply VS Code diagnostic settings (they override knot.json defaults)
    let mut inner = state.inner.write().await;
    for (i, (diag_key, _)) in diag_keys.iter().enumerate() {
        if let Some(value) = config_values.get(i)
            && let Some(severity_str) = value.as_str() {
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

    // Re-run analysis and publish diagnostics with updated config
    let diagnostics = AnalysisEngine::analyze(&inner.workspace);
    let open_docs = inner.open_documents.clone();
    let fmt_diags = inner.format_diagnostics.clone();
    let config = inner.workspace.config.clone();
    drop(inner);

    helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
}

pub(crate) async fn did_change_watched_files(state: &ServerState, params: DidChangeWatchedFilesParams) {
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
                if let Ok(path) = uri.to_file_path()
                    && let Ok(text) = std::fs::read_to_string(&path) {
                        let mut inner = state.inner.write().await;
                        let format = inner.workspace.resolve_format();
                        let (doc, parse_result) =
                            helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format, 0);

                        inner.open_documents.insert(uri.clone(), text.clone());
                        inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);
                        helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                        inner.workspace.insert_document(doc);
                        helpers::rebuild_graph(&mut inner.workspace);

                        let diagnostics = AnalysisEngine::analyze(&inner.workspace);
                        let open_docs = inner.open_documents.clone();
                        let fmt_diags = inner.format_diagnostics.clone();
                        let config = inner.workspace.config.clone();
                        drop(inner);

                        helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
                    }
            }
            FileChangeType::DELETED => {
                tracing::info!("File deleted: {}", uri);
                let mut inner = state.inner.write().await;
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

                helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;

                // Clear diagnostics for the deleted file
                state.client
                    .publish_diagnostics(uri, Vec::new(), None)
                    .await;
            }
            FileChangeType::CHANGED => {
                tracing::info!("File changed on disk: {}", uri);
                // Re-read and re-index the file if it's not currently open
                // (open files are tracked by did_change)
                let is_open = {
                    let inner = state.inner.read().await;
                    inner.open_documents.contains_key(&uri)
                };

                if !is_open
                    && let Ok(path) = uri.to_file_path()
                        && let Ok(text) = std::fs::read_to_string(&path) {
                            let mut inner = state.inner.write().await;
                            let format = inner.workspace.resolve_format();
                            let (doc, parse_result) =
                                helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format, 0);

                            inner.open_documents.insert(uri.clone(), text.clone());
                            inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);
                            helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                            inner.workspace.insert_document(doc);
                            helpers::rebuild_graph(&mut inner.workspace);

                            let diagnostics = AnalysisEngine::analyze(&inner.workspace);
                            let open_docs = inner.open_documents.clone();
                            let fmt_diags = inner.format_diagnostics.clone();
                            let config = inner.workspace.config.clone();
                            drop(inner);

                            helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
                        }
            }
            _ => {}
        }
    }
}
