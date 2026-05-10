//! Document synchronization handlers: did_open, did_change, did_close,
//! did_save, did_change_configuration, did_change_watched_files.

use crate::handlers::helpers;
use crate::state::ServerState;
use knot_core::editing::graph_surgery;
use knot_core::passage::Passage;
use lsp_types::*;
use url::Url;

pub(crate) async fn did_open(state: &ServerState, params: DidOpenTextDocumentParams) {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    let text = params.text_document.text;
    let version = params.text_document.version;

    tracing::info!("did_open: {}", uri);

    let mut inner = state.inner.write().await;

    // Clean up any stale URI-equivalent entries from workspace indexing.
    // We collect stale keys first to avoid double mutable borrow issues.
    let stale_keys: Vec<Url> = {
        let canonical_path = uri.to_file_path().ok();
        match canonical_path {
            Some(path) => inner.open_documents.keys()
                .filter(|k| **k != uri)
                .filter(|k| k.to_file_path().map_or(false, |p| p == path))
                .cloned()
                .collect(),
            None => Vec::new(),
        }
    };
    for key in &stale_keys {
        tracing::debug!("Removing stale URI-equivalent entry: {} (canonical: {})", key, uri);
        inner.open_documents.remove(key);
        inner.format_diagnostics.remove(key);
    }

    inner.editor_open_docs.insert(uri.clone());
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
    tracing::info!(
        passage_count = inner.workspace.get_document(&uri)
            .map(|d| d.passages.len()).unwrap_or(0),
        passages = ?inner.workspace.get_document(&uri)
            .map(|d| d.passages.iter().map(|p| format!("{}(links={},vars={},special={})", 
                p.name, p.links.len(), p.vars.len(), p.is_special)).collect::<Vec<_>>())
            .unwrap_or_default(),
        "did_open: passages defined"
    );
    let format = inner.workspace.resolve_format();
    inner.workspace.graph = helpers::rebuild_graph(&inner.workspace, &inner.format_registry, format);

    let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
    let open_docs = inner.open_documents.clone();
    let fmt_diags = inner.format_diagnostics.clone();
    let config = inner.workspace.config.clone();
    drop(inner);

    helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
}

pub(crate) async fn did_change(state: &ServerState, params: DidChangeTextDocumentParams) {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
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

    // Always update the text cache immediately so go-to-definition etc.
    // see the latest content
    inner.open_documents.insert(uri.clone(), text.clone());

    // Record the edit after updating the text cache, but do not skip the
    // parse/analysis pass.
    inner.debounce.record_edit();

    if inner.debounce.needs_flush() {
        inner.debounce.clear_skipped();
    }

    // Parse with format plugin
    let format = inner.workspace.resolve_format();
    let (doc, parse_result) =
        helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format.clone(), version);

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
    tracing::debug!(
        file = %uri,
        old_passage_count = old_passages.len(),
        old_passages = ?old_passages.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        "did_change: comparing old vs new passages"
    );
    let new_passages = doc.passages.clone();

    // Compute dynamic navigation edges for the new passages
    let extra_edges: Vec<(String, Option<String>, String)> = if let Some(plug) = inner.format_registry.get(&format) {
        let var_string_map = plug.build_var_string_map(&inner.workspace);
        new_passages.iter()
            .flat_map(|p| {
                plug.resolve_dynamic_navigation_links(p, &var_string_map)
                    .into_iter()
                    .map(|link| (p.name.clone(), link.display_text, link.target))
                    .collect::<Vec<_>>()
            })
            .collect()
    } else {
        Vec::new()
    };

    // Check for StoryData changes
    helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);

    inner.workspace.insert_document(doc);
    let file_uri_str = uri.to_string();
    let surgery_result = graph_surgery(
        &mut inner.workspace.graph,
        &old_passages,
        &new_passages,
        &file_uri_str,
        &extra_edges,
    );
    tracing::debug!(
        "graph_surgery result: added={:?} removed={:?} modified={:?}, graph nodes={} edges={}",
        surgery_result.added,
        surgery_result.removed,
        surgery_result.modified,
        inner.workspace.graph.passage_count(),
        inner.workspace.graph.edge_count()
    );

    // Update broken-link flags on all edges after surgery
    inner.workspace.graph.recheck_broken_links();

    let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
    tracing::debug!(
        file = %uri,
        diagnostic_count = diagnostics.len(),
        workspace_total_passages = inner.workspace.passage_count(),
        workspace_total_documents = inner.workspace.document_count(),
        graph_nodes = inner.workspace.graph.passage_count(),
        graph_edges = inner.workspace.graph.edge_count(),
        "did_change: analysis complete, workspace state dump"
    );

    // Log all passage definitions across the workspace for debugging
    // duplicate detection issues
    {
        let mut all_passage_names: Vec<(String, String)> = Vec::new();
        for d in inner.workspace.documents() {
            for p in &d.passages {
                all_passage_names.push((p.name.clone(), d.uri.to_string()));
            }
        }
        tracing::debug!(
            all_passages = ?all_passage_names,
            "did_change: full workspace passage definitions"
        );
    }
    let open_docs = inner.open_documents.clone();
    let fmt_diags = inner.format_diagnostics.clone();
    let config = inner.workspace.config.clone();
    drop(inner);

    helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
}

pub(crate) async fn did_close(state: &ServerState, params: DidCloseTextDocumentParams) {
    let uri = helpers::normalize_file_uri(&params.text_document.uri);
    tracing::info!("did_close: {}", uri);

    let mut inner = state.inner.write().await;
    // Remove from editor-open set only; keep text in open_documents cache
    // so that features like find-references still work for closed files
    inner.editor_open_docs.remove(&uri);
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
    let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
    let open_docs = inner.open_documents.clone();
    let fmt_diags = inner.format_diagnostics.clone();
    let config = inner.workspace.config.clone();
    drop(inner);

    helpers::publish_all_diagnostics(&state.client, &diagnostics, &fmt_diags, &open_docs, &config).await;
}

pub(crate) async fn did_change_watched_files(state: &ServerState, params: DidChangeWatchedFilesParams) {
    tracing::info!("did_change_watched_files: {} events", params.changes.len());

    for event in params.changes {
        let uri = helpers::normalize_file_uri(&event.uri);
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

                        // Remember the current format before inserting the new document
                        let format_before = inner.workspace.resolve_format();

                        let format = inner.workspace.resolve_format();
                        let (doc, parse_result) =
                            helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format.clone(), 0);

                        inner.open_documents.insert(uri.clone(), text.clone());
                        inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);
                        helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                        inner.workspace.insert_document(doc);

                        // If the new file's StoryData changed the format, re-parse
                        // ALL existing documents with the updated format.
                        let format_after = inner.workspace.resolve_format();
                        if format_before != format_after {
                            tracing::info!(
                                "Format changed from {:?} to {:?} after file creation — re-parsing all documents",
                                format_before, format_after
                            );
                            // Re-parse all documents with the new format
                            let uris: Vec<url::Url> = inner.open_documents.keys().cloned().collect();
                            let texts: Vec<String> = uris.iter()
                                .filter_map(|u| inner.open_documents.get(u).cloned())
                                .collect();

                            for (doc_uri, doc_text) in uris.iter().zip(texts.iter()) {
                                // Skip the newly created file — it's already parsed above
                                if *doc_uri == uri {
                                    continue;
                                }
                                let (re_parsed, re_result) =
                                    helpers::parse_with_format_plugin(&inner.format_registry, doc_uri, doc_text, format_after.clone(), 0);
                                inner.format_diagnostics.insert(doc_uri.clone(), re_result.diagnostics);
                                helpers::extract_and_set_metadata(&mut inner.workspace, &re_parsed, doc_text);
                                inner.workspace.insert_document(re_parsed);
                            }
                        }

                        inner.workspace.graph = helpers::rebuild_graph(&inner.workspace, &inner.format_registry, format_after);

                        let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
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
                inner.editor_open_docs.remove(&uri);
                inner.format_diagnostics.remove(&uri);
                inner.workspace.remove_document_and_update_graph(&uri);

                // Recheck broken links after removal
                inner.workspace.graph.recheck_broken_links();

                let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
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
                // Re-read and re-index the file ONLY if it's NOT currently
                // open in the editor. When a file is open, the did_change
                // handler manages updates from the editor buffer.
                let is_editor_open = {
                    let inner = state.inner.read().await;
                    inner.editor_open_docs.contains(&uri)
                };

                if !is_editor_open
                    && let Ok(path) = uri.to_file_path()
                        && let Ok(text) = std::fs::read_to_string(&path) {
                            let mut inner = state.inner.write().await;
                            let format = inner.workspace.resolve_format();
                            let (doc, parse_result) =
                                helpers::parse_with_format_plugin(&inner.format_registry, &uri, &text, format.clone(), 0);

                            inner.open_documents.insert(uri.clone(), text.clone());
                            inner.format_diagnostics.insert(uri.clone(), parse_result.diagnostics);
                            helpers::extract_and_set_metadata(&mut inner.workspace, &doc, &text);
                            inner.workspace.insert_document(doc);
                            inner.workspace.graph = helpers::rebuild_graph(&inner.workspace, &inner.format_registry, format);

                            let diagnostics = helpers::analyze_with_format_vars(&inner.workspace, &inner.format_registry);
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
